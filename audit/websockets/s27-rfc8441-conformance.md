# RFC 8441 (Bootstrapping WebSockets with HTTP/2) — conformance evidence

Independent verifier. Subject:
`crates/lb-l7/src/h2_proxy.rs::handle_ws_extended_connect` + the gating in
`H2Proxy::serve_connection`/`handle` + `crates/lb-l7/src/ws_proxy.rs`
(detector / relay). Branch `feature/websockets-s27`.

UPDATED at INC-5V (fixes `bd3f991d` INC-3, `1a308ac3` recon, `09ab157d`
INC-5 gating, `bc162fb8` INC-4). Supersedes the INC-2V version (line numbers
moved; the §4 `:scheme`/`:path` PARTIAL is now PASS; §3 is now gated/opt-in).

Verdict legend: PASS = met + evidenced; PARTIAL = met-in-practice with a
documented caveat; DEVIATION = deliberate non-implementation with rationale.

## Security disposition (read first)

WS-over-H2 is **OFF by default** (`lb_config::WebsocketConfig.h2_extended_connect`
default `false`, `lb-config/src/lib.rs:725,738`; wired
`lb/src/main.rs:1020` → `H2Proxy::with_h2_extended_connect`; field defaults
`false` at both `H2Proxy` ctors, `h2_proxy.rs:516,554`). The reason is
CF-S27-2: the WS-over-H2 tunnel does NOT propagate end-to-end backpressure
(unbounded gateway memory under a non-reading client). The capability is
opt-in for trusted-client deployments only. Both the SETTINGS advertisement
(`h2_proxy.rs:841`) and the extended-CONNECT intercept (`h2_proxy.rs:1055`)
are gated on `self.h2_extended_connect_enabled`.

| RFC 8441 requirement | Status | Evidence (test / file:line) |
|---|---|---|
| **§3** A server that supports extended CONNECT MUST send `SETTINGS_ENABLE_CONNECT_PROTOCOL` (0x8=1) in its initial SETTINGS — **AND, here, ONLY when opted-in.** | PASS (gated) | Advertised via `builder.enable_connect_protocol()` ONLY inside `if self.h2_extended_connect_enabled` (`h2_proxy.rs:841-843`). Gate-OFF proof (real-wire, reads the negotiated bit): `tests/ws_h2_gated_off.rs::connect_protocol_not_advertised_when_gated_off` asserts `!h2c.is_extended_connect_protocol_enabled()`. Gate-ON proof: `tests/ws_h2_e2e.rs` (a raw `h2` client only issues the extended CONNECT because the server advertised the setting). Code-presence guard for the gate: `crates/lb-l7/tests/h2_connect_protocol_settings.rs` (asserts `if self.h2_extended_connect_enabled` + pins id `0x8`). |
| **§4** Client sends extended CONNECT: `:method=CONNECT` + `:protocol=websocket`. Server detects + accepts. | PASS | Detector `ws_proxy.rs:213 is_h2_extended_connect` (CONNECT + `hyper::ext::Protocol`=="websocket", case-insensitive). Dispatched (gated) at `h2_proxy.rs:1055-1061`. Real-wire: `tests/ws_h2_e2e.rs` (status 200 + round-trip), `tests/ws_h2_conformance.rs::well_formed_extended_connect_tunnels_200`. |
| **§4** The extended CONNECT MUST include `:scheme` and `:path` (unlike classic CONNECT). | PASS (was PARTIAL at INC-2V) | INC-4 (`bc162fb8`): missing `:path` → 400 BEFORE any dial (`h2_proxy.rs:1399-1409`); missing `:scheme` → 400 BEFORE any dial (`h2_proxy.rs:1410-1416`). The prior silent `/` default is GONE. Proven real-wire by `tests/ws_h2_conformance.rs::extended_connect_missing_scheme_is_400` and `::extended_connect_missing_path_rejected_no_dial` (the latter also asserts the backend is NEVER dialed — load-bearing). |
| **§5** A `2xx` establishes the tunnel; the request stream becomes opaque bidirectional bytes. | PASS | 200 returned ONLY after the upstream WS handshake (`h2_proxy.rs:1530-1538`), stream wrapped via `server_ws` + relayed by `proxy_frames` (`h2_proxy.rs:1524-1525`). Real-wire round-trip (Text + 4 KiB Binary + clean Close): `tests/ws_h2_e2e.rs::ws_over_h2_text_binary_roundtrip_then_close`. |
| **§5 / error handling** Upstream failure → non-2xx; MUST NOT signal 2xx prematurely. | PASS (F-S27-1 fixed) | Inline dial + RFC 6455 handshake BEFORE any 200: refused→**502** (`h2_proxy.rs:1484-1490`), dial fail/budget elapsed→**504** (`h2_proxy.rs:1491-1504`), success→200 (`1530-1538`). Proven by `tests/ws_h2_upgrade_defer.rs` (502/504/no-smuggle), independently load-bearing (RED on pre-fix `e0e5a21b`, GREEN on fix — `inc2v-independent-loadbearing.txt`). Re-verified intact post-INC-3/4/5 (×3). |
| **§5** `ws`/`wss` scheme semantics; upstream contacted as a WebSocket. | PASS | URI `ws://{backend_addr}{path_and_query}` + `client_async_with_config` (`h2_proxy.rs:1453,1472-1478`). v1 dials the H1 backend over plaintext pooled TCP (`ws://`); backend `wss://` out of v1 scope (matches H1 sibling). |
| **§5.1** No `Upgrade`/H1 tokens on the H2 stream; framing via H2 DATA, not `101`. | PASS | The H2 path emits `200` and relays inside H2 DATA frames; never `101`. |
| **§4** `:protocol` matching case-insensitive. | PASS | `ws_proxy.rs:213` (`eq_ignore_ascii_case("websocket")`). |
| **W3C trace context parity** (ROUND8-OPS-06 / R12) — upstream WS handshake carries the CHILD `traceparent`. | PASS (INC-4) | `h2_proxy.rs:1427-1471` re-emits the child `traceparent`/`tracestate` on the upstream `ClientRequestBuilder`. Proven by `tests/ws_h2_conformance.rs::upstream_ws_handshake_carries_child_traceparent`: recorded `00-0af7651916cd43dd8448eb211c80319c-a398fd5e41686646-01` — trace-id preserved (== client), parent-id `a398...6646` != client's `b7ad...3331` (LB span is the new parent). |
| Per-message compression (RFC 7692). | DEVIATION (documented) | Deliberately NOT negotiated (`ws_proxy.rs` rationale). Avoids a decompression-bomb amplification surface. |
| WebSocket-over-QUIC / HTTP/3 (RFC 9220). | DEVIATION (documented) | Not implemented on the H3/quiche path. When built it MUST adopt the F-S27-1 deferred-success ordering AND avoid the CF-S27-2 backpressure gap from the start. |

## Residual / carried

1. **CF-S27-2 (carried, owner-dispositioned):** WS-over-H2 tunnel has no
   end-to-end backpressure → gated OFF by default. See
   `s27-r8-ws-proof.md` (corrected) + `s27-fs27-2-proof/`. NOT a conformance
   defect (RFC 8441 says nothing about flow-control propagation across the
   tunnel); it is a DoS posture, handled by the gate.
2. INC-4 note: missing `:scheme` is enforced by THIS handler's explicit
   check (hyper's h2 server does not require `:scheme` for extended CONNECT);
   missing `:path` is additionally RST_STREAM'd by hyper's codec, and the
   handler check is defense-in-depth + the single point that removed the old
   `/` default. Both verified real-wire.

## Summary

All mandatory RFC 8441 server requirements for the (opt-in) extended-CONNECT
WebSocket bootstrap are met and evidenced. The INC-2V `:scheme`/`:path`
PARTIAL is now PASS (INC-4). §3 advertisement is correctly conditioned on
the opt-in gate. The DoS gap (CF-S27-2) is carried and mitigated by the
off-by-default gate; it is not an RFC 8441 conformance defect.
