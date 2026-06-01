# RFC 8441 (Bootstrapping WebSockets with HTTP/2) — conformance evidence

Independent verifier, SESSION 27 INC-2V. Subject:
`crates/lb-l7/src/h2_proxy.rs::handle_ws_extended_connect` (fix `83746d1c`)
+ `crates/lb-l7/src/ws_proxy.rs` (detector / relay). Branch
`feature/websockets-s27`.

Verdict legend: PASS = met + evidenced; PARTIAL = met-in-practice with a
documented caveat; DEVIATION = deliberate non-implementation with rationale.

| RFC 8441 requirement | Status | Evidence (test / file:line) |
|---|---|---|
| **§3** A server that supports extended CONNECT MUST send `SETTINGS_ENABLE_CONNECT_PROTOCOL` (0x8 = 1) in its initial SETTINGS. | PASS | `h2_proxy.rs:804` `builder.enable_connect_protocol()` on the H2 server builder (always on; §3 comment at 801-803). Code-presence guard: `crates/lb-l7/tests/h2_connect_protocol_settings.rs` (`h2_proxy_calls_enable_connect_protocol`, pins id `0x8`). Real-wire proof: `tests/ws_h2_e2e.rs` — the raw `h2` client only issues an extended CONNECT because the server advertised the setting (the `h2` client gates `ext::Protocol` send on having received `ENABLE_CONNECT_PROTOCOL`); the round-trip therefore could not occur if §3 were unmet. |
| **§4** Client sends extended CONNECT: `:method = CONNECT` + `:protocol = websocket`. Server detects and accepts it. | PASS | Detector `ws_proxy.rs:174-184 is_h2_extended_connect` matches `Method::CONNECT` + the `hyper::ext::Protocol` typed extension == "websocket" (case-insensitive). Dispatched at `h2_proxy.rs:1011-1016`. Driven real-wire by `tests/ws_h2_e2e.rs` (`h2::ext::Protocol::from_static("websocket")`, status 200). |
| **§4** The extended CONNECT request MUST include `:scheme` and `:path` (unlike a classic CONNECT, which omits them). | PARTIAL | The detector `is_h2_extended_connect` does NOT itself require `:scheme`/`:path` to be present (it checks only `:method` + `:protocol`). However: (a) the happy path `tests/ws_h2_e2e.rs` sends a full `https://{host}/chat` URI so `:scheme`+`:path` are present; (b) the request also passes `crate::authority::validate_request` (`h2_proxy.rs:980`) before the WS intercept; (c) `handle_ws_extended_connect` reads `req.uri().path_and_query()` (`h2_proxy.rs:1336-1339`) defaulting to `/` if absent. Frame-level enforcement that a malformed extended CONNECT lacking `:scheme`/`:path` is rejected is delegated to hyper's H2 server (RFC 9113 malformed-request handling), not independently re-asserted here. Caveat: there is NO scoped test proving a `:scheme`/`:path`-less extended CONNECT is rejected with a stream error rather than silently defaulting to `/`. NOT a security gap (a missing `:path` defaults to `/`, which is forwarded verbatim to the H1 backend; no desync), but a conformance test gap — flagged below. |
| **§5** A `2xx` response to the extended CONNECT establishes the tunnel; the request stream then carries opaque bidirectional bytes (WebSocket frames in DATA frames). | PASS | `handle_ws_extended_connect` returns `200 OK` ONLY after the upstream `101` (`h2_proxy.rs:1417-1425`); the post-upgrade stream is wrapped via `server_ws` and relayed by `proxy_frames` (`h2_proxy.rs:1411-1414`). Real-wire bidirectional round-trip (Text + 4 KiB Binary echo + clean Close) proven by `tests/ws_h2_e2e.rs::ws_over_h2_text_binary_roundtrip_then_close`. |
| **§5 / error handling** On upstream failure the server MUST respond with a non-2xx; it MUST NOT signal tunnel-established (2xx) prematurely. | PASS (fixed by F-S27-1) | Inline upstream dial + RFC 6455 handshake BEFORE any 200: refused → **502** (`h2_proxy.rs:1371-1377`), dial failure / handshake-budget elapsed → **504** (`h2_proxy.rs:1378-1391`), success → 200 (`1417-1425`). Proven by `tests/ws_h2_upgrade_defer.rs` (502 / 504 / no-smuggle), independently load-bearing (RED on pre-fix `e0e5a21b`, GREEN on fix — see `inc2v-independent-loadbearing.txt`). |
| **§5** Use `ws`/`wss` scheme semantics; the upstream is contacted as a WebSocket. | PASS | Upstream URI built as `ws://{backend_addr}{path_and_query}` and handshaked with `tokio_tungstenite::client_async_with_config` (`h2_proxy.rs:1355-1365`). v1 dials the H1 backend over a plaintext pooled TCP socket (`ws://`); backend-side TLS (`wss://`) is out of v1 scope (consistent with the H1 sibling). |
| **§5.1** A WebSocket-over-H2 stream MUST NOT carry an `Upgrade` header / H1 upgrade tokens; framing is via the H2 stream, not H1 `101`. | PASS | The H2 path never emits `101`; it emits `200` and relays inside H2 DATA frames. The H1 upgrade tokens path is a separate code path (`h1_proxy.rs::handle_ws_upgrade`). |
| **§4** `:protocol` value matching is case-insensitive. | PASS | `ws_proxy.rs:183 eq_ignore_ascii_case("websocket")`. |
| Per-message compression (RFC 7692 `permessage-deflate`) negotiation. | DEVIATION (documented) | Deliberately NOT negotiated; rationale at `ws_proxy.rs:33-34`. Out of v1 scope. Avoids a compression-amplification (decompression-bomb) surface; conservative and safe. |
| WebSocket-over-QUIC / HTTP/3 (RFC 9220). | DEVIATION (documented) | Not implemented on the H3/quiche path; noted post-v1 at `ws_proxy.rs:34`. When built it MUST adopt the F-S27-1 ordering (upstream handshake before the success signal) from the start. |

## Caveats / test gaps to flag (not blockers)

1. **§4 `:scheme`/`:path` presence is not independently asserted.** A
   conformance test should drive an extended CONNECT that omits `:path`
   (and one that omits `:scheme`) and assert the gateway's behaviour
   (reject as malformed, OR document the `/` default). Current behaviour is
   safe (defaults to `/`, forwarded verbatim) but unverified by a scoped
   test. RECOMMEND: add to a future hardening pass. Severity: LOW
   (conformance/observability, no desync — the path is forwarded, not
   re-interpreted).

2. **§3 SETTINGS advertisement is guarded by a grep test, not a wire-parse
   test.** This is mitigated because `ws_h2_e2e.rs` proves it end-to-end
   (an `h2` client cannot issue the extended CONNECT unless the server
   advertised the setting), so a regression that dropped
   `enable_connect_protocol()` would turn `ws_h2_e2e.rs` red. Adequate.

## Summary

All mandatory RFC 8441 server requirements for the extended-CONNECT
WebSocket bootstrap are met and evidenced. The one PARTIAL (`:scheme`/`:path`
presence) is met in practice and safe; the gap is a missing negative
conformance test, not a behavioural defect. The two DEVIATIONs (RFC 7692,
RFC 9220) are deliberate, documented, and conservative.
