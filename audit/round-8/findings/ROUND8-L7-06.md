### ROUND8-L7-06 — No `keepalive_requests` count cap (Pingora 0.8.0 catch-up, nginx default 100)

Reference: `audit/round-8/research/pingora.md` lesson 12 (Pingora 0.8.0 added per-connection reuse-count cap as a new feature); `audit/round-8/research/nginx.md` lesson 16 (`keepalive_requests = 100` is the established edge norm); `audit/round-8/research/hyper-h2-quinn.md` hyper builder docs (no per-request count knob on the server). `ref-l7` handoff Top-10 #1: keepalive must have both wall-clock AND request-count caps.
Our equivalent: `crates/lb-l7/src/h1_proxy.rs:476-479` (hyper http1 Builder — `keep_alive(true)` only, no request-count cap), `crates/lb-l7/src/h2_proxy.rs:361` (timer wired, no per-conn count cap), `crates/lb-config/src/lib.rs:582-585` (only `keep_alive_interval_ms` / `keep_alive_timeout_ms`)

Severity: medium
Status:   Verified-Fixed-Partial(verifier=verify, 0575e5ac)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): H1 half VERIFIED — max_keepalive_requests counter (Arc<AtomicU64> shared across hyper service clones), Connection: close on cap-th response, ADDITIVE select! arm (drain arms untouched). round8_keepalive_count_cap 3/3 PASS + round8_drain_15case 16/16 PASS (no drain regression — the key risk). H2 lifetime-stream-cap->GOAWAY + Prometheus registry-lift HONESTLY DEFERRED & documented; status stays partial pending H2 follow-up. See audit/round-8/verify/l7.md. -->
Status (pre-fix):   Open

Divergence:
- **Reference (nginx)**: `keepalive_requests = 100` default — every keepalive connection is closed after 100 requests regardless of activity. Defence against long-tail memory / accounting accumulation, TLS session ageing, FD pinning.
- **Reference (Pingora)**: 0.8.0 added this as `keepalive_requests` cap *because Cloudflare hit the operational pain*. Industry-standard floor.
- **Us**: only `keep_alive_interval_ms` (PING period) and `keep_alive_timeout_ms` (PING-ACK deadline) exist. No request-count cap on either H1 or H2. A client that pipelines requests indefinitely will hold the connection forever as long as the PING ACKs land within timeout.

Impact:
- Memory accumulation: per-connection accounting (request log buffer, RFC 9110 logging context, slowloris detector state, watchdog entries) grows without bound across a long-lived keepalive.
- TLS session staleness: long keepalive means a single client session can outlive a TLS-ticket key rotation; the gateway loses the audit anchor that "this session was authenticated under key K1".
- Pool starvation against an attacker that opens N connections and pipelines forever — every other client must dial fresh.

Reproduction:
- Static-evidence: `crates/lb-l7/src/h1_proxy.rs:476-479`
  ```
  let conn = hyper::server::conn::http1::Builder::new()
      .keep_alive(true)
      .serve_connection(TokioIo::new(io), svc)
      .with_upgrades();
  ```
  No `.max_requests(...)` (hyper does not expose this; we would wrap by counting in `ProxyService::call`).
- Same shape in `h2_proxy.rs` — no streams-per-connection cap exists; we cap concurrent streams but not lifetime stream count.

Recommendation:
1. Add `[runtime].max_keepalive_requests` (default 100, matching nginx) to `lb-config`.
2. In `ProxyService::call` (H1 path), increment a per-connection counter held by the `ProxyService`'s clone source; when the count reaches the cap, add `Connection: close` to the response and trigger `disable_keep_alive` on the underlying connection. (Hyper exposes `disable_keep_alive` on the `Connection` future; thread a channel from the service back to the connection driver.)
3. For H2, set `max_concurrent_streams` already exists (256) — also cap *lifetime* streams; emit GOAWAY when reached.
4. Add an observability counter `lb_keepalive_terminated_by_count_cap_total{listener}`.
5. Pair-test with `auditor-delta` deliverable: the gate matrix should test that 101 requests on one keep-alive connection results in 100 served + a `Connection: close` on the 100th response.
