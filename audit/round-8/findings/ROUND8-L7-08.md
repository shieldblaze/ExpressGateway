### ROUND8-L7-08 — Upstream H2 read timeout drops future without explicit RST_STREAM(CANCEL) (Pingora 0.8.0 fix class)

Reference: `audit/round-8/research/pingora.md` lesson 10 (Pingora 0.8.0: "Send RST_STREAM CANCEL on application read timeouts for h2 client" — otherwise upstream keeps shoveling bytes into a closed stream). Defensive pattern #5: "RST_STREAM(CANCEL) on H2 upstream read timeout + mark connection unreusable. Pingora explicitly does both."
Our equivalent: `crates/lb-io/src/http2_pool.rs:199-210` (`send_request`, timeout branch), `crates/lb-l7/src/h2_to_h2.rs`, `crates/lb-l7/src/h1_to_h2.rs`

Severity: medium
Status:   Deferred-with-rationale(verifier=verify; lead-decision R8-L-002)   <!-- INDEPENDENT VERIFICATION (verifier=verify): deferral VERIFIED HONEST. hyper pinned 1.9.0 (Cargo.lock:1123); hyper-1.x SendRequest exposes no send_reset/explicit RST_STREAM(CANCEL) — accurate. Existing mitigation (pool eviction on every Send-class error + timeout) confirmed present at http2_pool.rs:206-222. Lead pre-ack R8-L-002. Re-open tracked for hyper-2.x. Residual risk (upstream sees uncoded resets) accepted. Not a rubber-stamp. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference**: on application-level read timeout, explicitly emit `RST_STREAM(CANCEL)` to the upstream and mark the connection unreusable (the timeout suggests the upstream hung on this stream specifically — the connection might be sick).
- **Us**: `http2_pool.rs:206-209` evicts the H2 connection from the pool on timeout — but does not send an explicit `RST_STREAM`. Dropping the `send_request` future causes hyper to emit a reset eventually, but the reset code is `NO_ERROR` (or `STREAM_CLOSED`) rather than `CANCEL`, and the timing is the next scheduling tick, not immediate. From the upstream's perspective the proxy looks indistinguishable from a misbehaving peer; the upstream's rapid-reset detector may count *us* as the attacker.

```rust
// http2_pool.rs:206-209
Err(_) => {
    self.evict(addr);                     // ← removes from pool, but other inflight streams on this conn are abandoned
    Err(Http2PoolError::Timeout)
}
```

Impact:
- Upstream observes us as a rapid-reset peer if many streams time out on the same connection. Some upstream H2 servers (Envoy with `max_pending_accept_reset_streams`) will then send GOAWAY against *us* — propagating back as 5xx to our clients.
- The evicted connection's *other* inflight streams (multiplexed) lose their multiplexer — depending on hyper's drop semantics, those streams either error immediately or hang until their own timeout. Pingora L#11 (panic on multi-stream concurrent use) is the recovery cousin.
- Connection-slot accounting: the upstream keeps the slot occupied until its own idle timer fires (typically 60s+), giving us *one* slot we cannot reuse during that window.

Reproduction:
- Static evidence: grep `crates/lb-io/src/http2_pool.rs` and `crates/lb-l7/src/h2_to_h2.rs` for `send_reset`, `CANCEL`, `reset_stream` — zero matches. Hyper's `SendRequest` does not expose `send_reset`; the only path to emit RST is via the request body channel.
- Crowd-test: open 256 H2 streams to a backend that holds them open; configured `send_timeout` fires for each; observe upstream sees 256 *resets* (good) but coded as `NO_ERROR`/cancellation-without-context (bad).

Recommendation:
1. On timeout in `http2_pool::send_request`, before evicting:
   - Use hyper's `client::ResponseFuture::cancel`-equivalent if available (hyper 1.x has been gradually exposing this; check the latest API).
   - If no API, the practical fallback is to send a zero-length body chunk that triggers immediate close-with-CANCEL semantics — but this is library-version-specific.
2. After the timeout, set the cached sender's `is_closed()` poll to true via `.disable_*` if exposed.
3. Add a counter `lb_h2_upstream_cancel_total{reason="read_timeout"}` so operators can chart this against upstream GOAWAYs.
4. Pingora's CHANGELOG 0.8.0 documents the exact fix shape; lift the pattern (not the code).
