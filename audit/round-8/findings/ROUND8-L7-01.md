### ROUND8-L7-01 — Premature 101 Switching Protocols on WebSocket upgrade (Pingora-class CVSS 9.3)

Reference: `audit/round-8/research/pingora.md` lesson 3 (Pingora GHSA-xq2h-p299-vjwv, Critical, CVSS 9.3) + `audit/round-8/research/envoy.md` lesson 5 (Envoy GHSA-rj35-4m94-77jh) — same bug class shipped by two independent codebases.
Our equivalent: `crates/lb-l7/src/h1_proxy.rs:902-959` (`handle_ws_upgrade`), `crates/lb-l7/src/h1_proxy.rs:996-1054` (`run_h1_ws_upgrade_task`)

Severity: high
Status:   Verified-Fixed(verifier=verify, 6253ad9a)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): clean rebuild; round8_ws_upgrade_defer 4/4 + round8_traceparent_propagation 1/1 + round8_drain_15case 16/16 PASS. Bypass: premature-101 race STRUCTURALLY IMPOSSIBLE (sole 101 ctor downstream of inline timeout(upstream_dial).await; pre-fix detached dial-after-101 task deleted); smuggled-pipeline reproducer closed; traceparent parent-id replaced not forwarded. Non-blocking caveat: Sec-WebSocket-Accept derived from client key not echoed from upstream (RFC6455-equivalent; upstream-negotiated extensions not reflected) — extension follow-on, NOT a security defect. See audit/round-8/verify/l7.md. -->
Status (pre-fix):   Open

Divergence:
- **Reference**: tunnel/upgrade bytes are forwarded to upstream *only after* the upstream returns `101 Switching Protocols` (WS) or a `2xx` (CONNECT). The protocol switch is decided by the upstream.
- **Us**: `h1_proxy::handle_ws_upgrade` emits the `101 Switching Protocols` response to the client **before** any dial to upstream. The detached `run_h1_ws_upgrade_task` only then `pool.acquire_async(backend_addr)` and `client_async_with_config(...)` to drive the upstream handshake. If the upstream handshake fails, we `tracing::debug!` and return — by which time the client has already received the 101 and committed to WS framing.

```text
// h1_proxy.rs:946-958
let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
for (name, value) in handshake_headers { builder = builder.header(name, value); }
// ...
builder.body(body)...  // returned to hyper → wire flips to WS
// → run_h1_ws_upgrade_task spawned: upstream dial happens AFTER 101 is on the wire.
```

Impact:
- Attacker request: `Upgrade: websocket` + a follow-on smuggled HTTP request on the keep-alive pipeline. We accept the upgrade unconditionally, write 101, then attempt upstream dial. If the upstream rejects the WS (or is unreachable), we tear down — but any bytes the attacker queued after the upgrade headers (read by hyper's `OnUpgrade`) have already entered the upgraded byte stream that no one is reading authoritatively yet. The hyper `OnUpgrade` future yields the raw byte stream including unread post-handshake bytes from the *client*; combined with a backend that is also still sending an H1 response, the framing is desynced.
- Operationally: WebSocket DoS — flood handshake requests against backends known to reject; LB returns 101 to every client, spawns a dial-and-fail task per request, accumulates `OnUpgrade` futures and `tokio::spawn` tasks until pool / FD pressure.
- The Pingora & Envoy fixes are identical in shape: do not respond 101 until the upstream `101` arrives. We do not implement this.

Reproduction:
- `crates/lb-l7/src/h1_proxy.rs:902-959` — read the function top-down: 101 is built and returned synchronously from `handle_ws_upgrade`; upstream dial is in the spawned `run_h1_ws_upgrade_task` which runs *after* hyper has flushed the 101 onto the wire.
- No integration test covers the case where the backend rejects the WS handshake; the existing `ws_proxy.rs` tests run only the success path.

Recommendation:
1. Restructure `handle_ws_upgrade` so the upstream handshake happens **before** the 101 is written. Hyper's `Service::call` allows an async return — `await` the backend `client_async_with_config` and only then build the 101 response.
2. On upstream-handshake failure, return `502 Bad Gateway` to the client (which is still in H1 mode because we haven't flipped the protocol). Add a regression test mirroring Pingora's GHSA-xq2h-p299-vjwv reproducer: client sends a smuggled second request after the WS upgrade headers; upstream returns non-2xx; assert the smuggled request is NOT forwarded.
3. CONNECT method is not implemented in `h1_proxy` (no match arm for `Method::CONNECT`) — this is a non-goal we should document in `audit/deferred.md` rather than leave silent.
