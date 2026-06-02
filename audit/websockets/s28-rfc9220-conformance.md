# RFC 9220 (Bootstrapping WebSockets with HTTP/3) — conformance evidence

Subject: WS-over-H3 Stage C — the conn_actor tunnel-mode pump
(`crates/lb-quic/src/conn_actor.rs`), the H3 extended-CONNECT validator
(`crates/lb-quic/src/h3_bridge.rs::validate_request_pseudo_headers`), the
SETTINGS advertisement (`crates/lb-quic/src/h3_config.rs::build_server_h3_config`),
and the injected relay launcher (`crates/lb/src/main.rs::build_ws_h3_launcher`
+ `lb_l7::ws_proxy::{dial_backend_ws, server_ws, proxy_frames}`). Branch
`feature/ws-h3-stagec-s28`.

RFC 9220 is short: it says WebSockets over HTTP/3 work exactly like RFC 8441
over HTTP/2, with the HTTP/2-specific pieces replaced by their HTTP/3
equivalents (SETTINGS_ENABLE_CONNECT_PROTOCOL is an HTTP/3 setting; the
Extended CONNECT uses the HTTP/3 request stream; DATA frames carry the
tunneled bytes). So the evidence below maps RFC 9220 → RFC 8441 §3–5.

Verdict legend: PASS = met + evidenced; PARTIAL = met-in-practice with a
documented caveat; DEVIATION = deliberate non-implementation with rationale.

## Security / availability disposition (read first)

WS-over-H3 is **OFF by default** (`lb_config::WebsocketConfig.h3_extended_connect`
default `false`; wired in `lb/src/main.rs::spawn_quic` →
`QuicListenerParams::with_websocket` only when the `[listeners.websocket]`
block has `enabled && h3_extended_connect`). Unlike WS-over-H2's gate
(CF-S27-2, a real DoS/backpressure caveat), the H3 gate is a **NEWNESS gate**:
H3 backpressures end-to-end **by construction** (the bounded `H3WsTunnel`
`PollSender` parks the relay, and quiche's per-stream send buffer is capped at
the flow-control window — proven R8, below). It is opt-in only because it is a
new feature on the freshly-migrated quiche::h3 stack and is proving out.

When OFF: the H3 SETTINGS frame omits `SETTINGS_ENABLE_CONNECT_PROTOCOL`, the
`:protocol` pseudo-header is rejected by the validator, and the front is
byte-identical to the pre-S28 server (R3, proven by the lb-quic suite passing
with `ws_relay_launcher: None` threaded everywhere).

| RFC 9220 / RFC 8441 requirement | Status | Evidence (test / file:line) |
|---|---|---|
| **§3** A server supporting extended CONNECT MUST send `SETTINGS_ENABLE_CONNECT_PROTOCOL` (=1) in its initial SETTINGS — here ONLY when opted-in. | PASS (gated) | `h3_config.rs::build_server_h3_config(ws_enabled)` calls `cfg.enable_extended_connect(true)` ONLY under `if ws_enabled`. quiche emits the setting in the H3 SETTINGS frame (`quiche-0.28 h3/mod.rs:617,651`). OFF ⇒ not advertised ⇒ byte-identical frame (R3). The real-wire e2e (`ws_over_h3_extended_connect_echo_roundtrip_through_real_listener`) only succeeds because a `:protocol=websocket` request is accepted — i.e. the capability is live when ON. |
| **§4** Client sends Extended CONNECT: `:method=CONNECT` + `:protocol=websocket`. Server detects + accepts. | PASS | Validator accepts `:protocol` under `ws_enabled` (`h3_bridge.rs:538`); `poll_h3` intercepts `method==CONNECT` + `:protocol` present and routes to `setup_ws_tunnel` (`conn_actor.rs`, Event::Headers arm). `:protocol` matched case-insensitively vs `websocket` (`setup_ws_tunnel`: `protocol.eq_ignore_ascii_case("websocket")`). Real-wire: `ws_over_h3_..._echo_roundtrip` (status 200 + Text round-trip through `spawn_quic`). |
| **§4** Extended CONNECT MUST include `:scheme` + `:path` + `:authority` (unlike classic CONNECT, which omits `:scheme`/`:path`). | PASS | `validate_request_pseudo_headers` (`h3_bridge.rs:571-583`): under `seen_protocol`, missing `:scheme`/`:path`/`:authority` → `H3_MESSAGE_ERROR` reset, BEFORE the tunnel is built or any backend dialed. Classic CONNECT (no `:protocol`) keeps the inverted rule (`:585-592`). Validator unit tests in `h3_bridge.rs` (S27 Stage A) cover the accept/reject matrix; the e2e sends all five pseudo-headers. |
| **§4** Unknown `:protocol` (≠ websocket) → reject, do not tunnel. | PASS | `setup_ws_tunnel`: a `:protocol` that is not `websocket` → inline **501 Not Implemented** ("unsupported :protocol") via `spawn_inline_h3_response`, no tunnel built. (RFC 8441 §4 leaves the status to the server; 501 is the registered "method/protocol not supported".) |
| **§5** A `2xx` establishes the tunnel; the request stream becomes opaque bidirectional bytes (DATA frames). | PASS | `pump_ws_tunnels` sends `send_response(200, …, fin=false)` ONLY on `WsUpstreamOutcome::Ready`, then relays: inbound `recv_body → to_reader`, outbound `from_writer → send_body` — opaque WS frames over H3 DATA. Real-wire round-trip (Text echo + clean Close): `ws_over_h3_..._echo_roundtrip`. |
| **§5 / error handling** Upstream failure → non-2xx; MUST NOT signal 2xx prematurely (no false success / no smuggle). | PASS | The launcher (`build_ws_h3_launcher`) dials the H1 backend + drives the upstream RFC 6455 handshake INLINE and signals readiness via a oneshot BEFORE the 200: refused → **502**, dial/handshake budget elapsed → **504** (`pump_ws_tunnels` `Failed{status}` → `spawn_inline_h3_response`). The H3 analog of the WS-H1 GHSA / WS-H2 F-S27-1 "upstream-before-200" ordering — correct from the start. (Cross-transport ordering confirmed in `s28-report.md` §upstream-before-200.) |
| **§5** `ws`/`wss` scheme; upstream contacted as a WebSocket. | PASS | Launcher builds `ws://{backend_addr}{:path}` + `client_async_with_config` (`ws_proxy.rs::dial_backend_ws`). v1 dials the H1 backend over plaintext pooled TCP (`ws://`), matching the H1/H2 siblings; backend `wss://` is out of v1 scope. |
| **§5.1** No `Upgrade`/H1 tokens; framing via H3 DATA, not `101`. | PASS | The H3 path emits `200` (via `send_response`) and relays inside H3 DATA frames; never `101`, never `Upgrade`. |
| **Reset / abnormal close mapping (RFC 9220 §3 ⇒ RFC 9114 reset semantics).** A WS abnormal drop over H3 must map to a stream reset, NOT a clean Close (truncation / desync guard). | PASS | `ws_handle_client_reset` / `ws_handle_client_fin` (Finished-on-reset probed via zero-length `stream_recv` → `StreamReset`) → `TunnelInbound::Reset` → `ConnectionReset` → `proxy_frames` errors → upstream dropped abruptly. Conversely a client FIN → clean EOF → WS Close forwarded. Proven (load-bearing negative control) by `ws_over_h3_reset_maps_to_abnormal_drop_not_clean_close`: clean→`CleanClose`, reset→`Abrupt`. Gateway-side teardown uses `H3_REQUEST_CANCELLED` (0x010c). |
| **End-to-end backpressure (availability).** The tunnel MUST be bounded-incremental, not buffering. | PASS (R8) | Bounded by construction: `H3WsTunnel` channels (depth 8 × 8 KiB ≈ 64 KiB each way), `out_pending` ≤ one chunk, quiche send buffer capped at the flow-control window (`quiche-0.28 stream/send_buf.rs::cap`). Re-proven on the WIRED tunnel: `ws_over_h3_outbound_backpressure_plateaus_then_drains` — a 512-frame flood at a stalled client PLATEAUS at ~63 (volume-independent), then drains fully on resume (liveness, no loss). Inbound is symmetric (`ws_drain_inbound` recv_body only while `to_reader` has capacity ⇒ QUIC flow control not extended ⇒ client paced), mirroring the proven request-body path. |
| **Subprotocol negotiation (RFC 6455 §1.3 / RFC 8441 §5).** | PASS | Client `sec-websocket-protocol` offer forwarded to the upstream (`dial_backend_ws` `with_sub_protocol`); the upstream's selected subprotocol echoed in the 200 (`WsUpstreamOutcome::Ready{headers}` → `pump_ws_tunnels` send_response headers). |
| Per-message compression (RFC 7692). | DEVIATION (documented) | Deliberately NOT negotiated (matches H1/H2 siblings; avoids a decompression-bomb amplification surface). |
| WS limits (idle / read-frame watchdog / ping-flood). | PASS (inherited) | The relay is the single-sourced `proxy_frames`, so the WsConfig idle-timeout (default 60 s), per-direction read-frame watchdog (30 s), and client-ping rate limit (50/10 s) apply identically to the H3 tunnel (the `H3WsTunnel` is just the IO). |

## Single-sourcing (R12)
- The frame relay is `lb_l7::ws_proxy::proxy_frames`, reused (not duplicated)
  across WS-H1, WS-H2, WS-H3 via the `H3WsTunnel` `AsyncRead+AsyncWrite` seam +
  the injected launcher closure (the dependency-inversion design).
- The "upstream-before-200" ordering is the same across all three transports
  (WS-H1 GHSA fix, WS-H2 F-S27-1, WS-H3 launcher). No divergence.
- The inline error-response path (`spawn_inline_h3_response`) is single-sourced
  and now also serves the pre-existing inline 400 (:authority reject).
