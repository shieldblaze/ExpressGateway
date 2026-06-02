# F-S27-1 — Premature 200 on WS-over-H2 (RFC 8441) extended CONNECT

Independent verifier, SESSION 27. Branch `feature/websockets-s27`.
Reviewed code authored by ws-eng (not the verifier).

## Verdict

**F-S27-1 CONFIRMED. Severity: HIGH (security).**

The WS-over-HTTP/2 path (`H2Proxy::handle_ws_extended_connect`) signals
WebSocket success (`200 OK`) to the client **before** the upstream RFC 6455
handshake is attempted. On a backend that passes the synchronous guards but
rejects the WS handshake, the client receives `200 OK` (observed) where a
correctly ordered gateway must return `502/504` (required). This is the
HTTP/2 analog of Pingora GHSA-xq2h-p299-vjwv / Envoy GHSA-rj35-4m94-77jh
(both CVSS 9.3), which was already FIXED for the HTTP/1 path. It is also an
R12 sibling-divergence: H1 fixed, H2 not.

## Code-path proof (exact file:line, current main)

`crates/lb-l7/src/h2_proxy.rs`:

- `fn handle_ws_extended_connect(&self, mut req)` is a **synchronous** `fn`
  returning `Response<ClientRespBody>` — declared at **line 1288**, NOT
  `async`, and it does not await the upstream at all.
- The only checks performed before responding are SYNCHRONOUS guards:
  - **1292-1293** ws disabled -> 502
  - **1295-1297** `self.picker.pick_info()` None -> 502 ("no backend
    available")
  - **1298-1303** backend proto != H1 -> 502 ("requires H1 backend")
  A real, pickable, H1-protocol backend that merely fails the *WS handshake*
  passes ALL THREE guards.
- **1306** `let upgrade_fut = hyper::upgrade::on(&mut req);` — the upgrade
  future is armed but NOT awaited here.
- **1314-1366** `tokio::spawn(async move { ... })` — the dial
  (`pool.acquire_async`, **1328**) and the upstream RFC 6455 handshake
  (`tokio_tungstenite::client_async_with_config`, **1348-1360**) run ONLY
  inside this detached task. The detached task runs after
  `hyper::upgrade::on` resolves — i.e. after the 200 response headers reach
  the wire.
- On upstream failure every arm in the detached task does
  `tracing::debug!(...); return;` (**1318-1320, 1331-1333, 1336-1337,
  1343-1345, 1357-1360**). There is NO path back to the client response:
  the 200 has already been built and returned.
- **1368-1377** the function unconditionally builds and returns
  `Response::builder().status(StatusCode::OK)` with an empty body. The 200
  is returned regardless of whether the upstream handshake will succeed,
  fail, or even be attempted.

Dispatch: `h2_proxy.rs:998-1004` routes a detected extended CONNECT into
`return self.handle_ws_extended_connect(req);` from `serve_connection`'s
request handler, and that `Response` is sent to the client by the hyper H2
server task as soon as it is returned. So the 200 reaches the wire before
the spawned task does any upstream work.

### Contrast with the FIXED H1 sibling

`crates/lb-l7/src/h1_proxy.rs::handle_ws_upgrade` (line 2409, `async fn`):
- **2443-2491** dials the upstream and drives `dial_upstream_ws(...)` under
  `tokio::time::timeout(self.timeouts.header, ...)` and returns
  `502` (Upstream), `504` (Timeout / Elapsed) on failure — BEFORE any
  client-visible response.
- **2493-2501** only AFTER the upstream handshake succeeds does it arm
  `hyper::upgrade::on` and build the client `101`.
The H2 path inverts this ordering.

## Mechanism demonstration (real-wire negative control)

Reproducer: `fs27_1_repro.rs.txt` (in this directory; deliberately NOT under
`tests/` so it does not compile into the committed suite). It mirrors the H1
sibling `crates/lb-l7/tests/round8_ws_upgrade_defer.rs`
(`client_sees_502_when_backend_rejects_ws`) over the WS-over-H2 path, reusing
the EXACT harness of `tests/ws_h2_e2e.rs` (real TLS, ALPN h2, raw `h2`
extended CONNECT via `h2::ext::Protocol`, `H2Proxy::with_websocket`).

Backend: a real `TcpListener` (a pickable H1 address — passes the synchronous
guards) that accepts the TCP connection, reads the inbound upstream handshake,
and answers `HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n` — a deliberate
NON-101 that REJECTS the WebSocket.

Two tests:
1. `client_must_not_see_200_when_upstream_rejects_ws` asserts the post-fix
   contract (status must be 502/504, never 200). On current main it **FAILS**
   — the client received `200 OK`.
2. `proof_200_is_premature_tunnel_is_dead` (load-bearing non-vacuity control)
   characterizes current main: the client gets `200 OK` AND then a real WS
   client round-trip over the "established" H2 stream yields NO echo
   (`got_echo = false`) — because the upstream rejected the handshake, so the
   relay never started. The 200 was a lie.

Captured console (`repro-console.txt`):

```
PROOF observed extended-CONNECT status = 200 OK
F-S27-1 OBSERVED extended-CONNECT status = 200 OK
thread 'client_must_not_see_200_when_upstream_rejects_ws' panicked ...:
assertion `left != right` failed: ... (got 200 OK); a fixed gateway returns 502/504 ...
  left: 200
 right: 200
PROOF round-trip over the promised tunnel got_echo = false
test proof_200_is_premature_tunnel_is_dead ... ok
test client_must_not_see_200_when_upstream_rejects_ws ... FAILED
test result: FAILED. 1 passed; 1 failed; ...
```

Observed (current main): **200 OK**, tunnel dead.
Required (fixed gateway): **502** (handshake refused) or **504** (dial/
handshake timeout); NEVER 200.

The non-vacuity is anchored two ways: (a) the load-bearing control shows the
promised tunnel is dead after the 200; (b) `tests/ws_h2_e2e.rs` runs the
IDENTICAL client/gateway machinery against a 101-capable echo backend and
DOES round-trip (would show `got_echo = true`). The sole difference between
the two is whether the upstream handshake succeeds — yet the client sees 200
in both cases. That decoupling IS the defect.

## Impact

A client is told the WebSocket is established before the gateway has
confirmed (or even attempted) the upstream. Consequences mirror the cited
CVEs: a client the upstream would have rejected is committed to WS framing on
a wire that then silently closes; clients cannot distinguish "connected" from
"about-to-be-severed"; and the premature switch into an opaque
upgraded/tunnel byte-stream is the request-smuggling primitive both
advisories paid for. Severity HIGH, matching the H1 fix that this path fails
to mirror (the H1 fix referenced CVSS 9.3).

## Post-fix contract (load-bearing for INC-2)

The INC-2 regression test (the committed green sibling of this reproducer)
MUST assert, over the same real-wire WS-over-H2 extended-CONNECT path with a
pickable H1 backend that passes the synchronous guards:

1. When the upstream answers the WS handshake with a NON-101 (e.g. `200`/
   `400`/`403`) or closes without 101: the client's extended-CONNECT
   response status is **502 Bad Gateway**, and is NEVER **200**.
2. When the upstream accepts the TCP connection but never completes the WS
   handshake within the bounded handshake budget: the client's status is
   **504 Gateway Timeout**, and is NEVER **200**.
3. (smuggling) No request/bytes pipelined by the client after the extended
   CONNECT may reach the backend when the upstream handshake is refused —
   the H2 analog of the H1 `no_smuggled_request_forwarded_on_failure`. For
   H2 this is naturally constrained (the stream stays in request state, no
   tunnel bytes are spliced), but assert it explicitly: the rejecting backend
   records its inbound bytes and must NOT contain any client-sent DATA-frame
   payload.
4. The happy path (`ws_h2_e2e.rs`) must STILL pass: a 101-capable backend
   yields **200** and a working Text/Binary/Close round-trip.

The fix must move the dial + upstream RFC 6455 handshake (under a bounded
timeout) to BEFORE the `200` is built (making `handle_ws_extended_connect`
`async`, mirroring H1's `handle_ws_upgrade` at h1_proxy.rs:2443-2501), and
map handshake-refused -> 502, dial/handshake-timeout -> 504. To confirm the
INC-2 regression test is itself load-bearing, REVERT the fix and re-run it:
it must go red (client sees 200) exactly as this reproducer does.

## R12 sibling survey

- H1 sibling **IS fixed**: `crates/lb-l7/src/h1_proxy.rs::handle_ws_upgrade`
  dials + handshakes before any client-visible response (2443-2501), proven
  by `crates/lb-l7/tests/round8_ws_upgrade_defer.rs`
  (`client_sees_502_when_backend_rejects_ws`,
  `client_sees_504_on_dial_timeout`,
  `no_smuggled_request_forwarded_on_failure`).
- WS-over-H3 does **not yet exist** in this codebase (no extended-CONNECT /
  WebSocket handling on the H3/quiche path). When it is built it MUST adopt
  the correct ordering from the start (upstream handshake before the success
  signal), so the same class of bug is never introduced.
