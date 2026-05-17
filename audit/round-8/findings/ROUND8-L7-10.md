### ROUND8-L7-10 — Body over-read does not mark connection non-reusable (Pingora 0.8.0 fix class)

Reference: `audit/round-8/research/pingora.md` lesson 8 (Pingora 0.8.0 fix: "Ensure http1 downstream session is not reused on more body bytes than expected") and lesson 20 (0.6.0 fix: "Discard extra upstream body and disable keepalive"). `ref-l7` Top-10 #1 — the *same* primitive in two directions.
Our equivalent: `crates/lb-l7/src/h1_proxy.rs:758-802` (`proxy_request`), `crates/lb-io/src/pool.rs:387-389` (`set_reusable`) — no caller sets it `false` on body over-read

Severity: medium
Status:   Verified-Fixed(verifier=verify, 9bd20cd9)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): H1 take-and-discard documented (no silent re-introduction), set_reusable doc-comment, http2_pool::send_request widened to evict on every Send-class error. round8_body_overread 4/4 PASS (incl. live h2_pool_failed_send_leaves_no_stale_entry). H1 single-use so over-read reuse structurally absent. Matches finding's medium/future-proofing framing. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference**: when either side sends more body bytes than the declared `Content-Length`, the connection MUST be marked unreusable on that side. Downstream over-read → don't keep-alive the *client* connection. Upstream over-read → evict the *upstream* from pool. (Two CHANGELOG entries because Pingora paid for the bug twice.)
- **Us**:
  - Downstream: hyper handles CL framing internally; if a client sends extra bytes, hyper detects the framing error and the request completes with an error. But we do not explicitly `disable_keep_alive` on the connection after such an error — the next request on the same keepalive connection inherits whatever hyper does. (Hyper *should* close on a framing error; verify by test.)
  - Upstream: `proxy_request` does not call `pooled.set_reusable(false)` on any error path. `PooledTcp::set_reusable` exists in `lb-io/src/pool.rs:389`. Grep for callers across `lb-l7/src` returns **zero matches**. Every pooled TCP connection is returned to the pool on drop unconditionally, regardless of whether the upstream response framing was clean.

```rust
// h1_proxy.rs:768-770
let stream = pooled
    .take_stream()
    .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;
// `pooled` is now without its stream; drop on it is a no-op for the inner Std stream which moved out.
```

We `take_stream()` immediately, so `pooled` is effectively a no-op wrapper that drops without returning anything to the pool. **All connection reuse is via the H2 `Http2Pool::peers` map, the H1 stream is single-use after take.** So the H1 case is actually safer than feared (we never reuse the upstream H1 socket via `set_reusable`). But this is by accident, not design — there is no comment explaining the take-and-discard pattern, and the next refactor that re-adds reuse will silently re-introduce the over-read bug.

Impact:
- H2 side: `Http2Pool::evict` on `Send` error covers the case, but only after the error returns. An over-read that hyper detects on a *separate stream* later (sharing the multiplexer) is not evicted.
- Future-proofing: the documented intention in `lb-io/src/pool.rs:387-389` of `set_reusable(false)` exists for a reason but no caller uses it. The day we wire H1 upstream-reuse, the over-read bug is back.

Reproduction:
- Static evidence: `grep -rn 'set_reusable' /home/ubuntu/Code/ExpressGateway/crates/lb-l7/src` returns nothing. `crates/lb-io/src/pool.rs:387-389` documents the API but no production caller.
- Behaviour: take an `Http2Pool` with a backend that returns a body shorter than its CL header. The pool's cached sender remains alive (`is_closed()` is false until the connection layer notices). Next `send_request` hits the same poisoned sender; hyper's stream-level error propagates but we don't drop the connection.

Recommendation:
1. Document the H1 take-and-discard pattern in `proxy_request` so it survives the next refactor.
2. In `Http2Pool::send_request`, on `Send` error, inspect the error class. If `e.is_h2_error()` and the reason is body-length related (`STREAM_CLOSED` mid-body), `evict` the connection (we already do this — but verify the matcher catches CL mismatches and isn't just timeout-keyed).
3. Add a regression test: configure a backend with `Connection: keep-alive` but emit `body.len() != CL`; assert the proxy does not reuse the connection on the next request.
4. Document the intent on `PooledTcp::set_reusable` — it's a public function with no production user.
