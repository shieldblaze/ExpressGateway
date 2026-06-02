# SESSION 28 — handoff (from S27, WebSockets)

S27 delivered WebSocket proxying over **H1 (RFC 6455)** and **H2 (RFC 8441, gated off-by-default)**,
plus the **WS-over-H3 (RFC 9220) groundwork** (Stages A+B). S28 finishes WS-over-H3 (Stage C) and
takes the next spec item (gRPC-over-H3). Base: main after the S27 promote (confirm tip first).

## 1. WS-over-H3 Stage C — FINISH THE RELAY (top priority; design APPROVED)
Stages A+B are on main (config+validator R3-safe; `ws_tunnel.rs` bounded adapter, R8-in-isolation +
reset-vs-EOF). Stage C wires them into the actor + real-wire-verifies. **~1 session-equivalent.**

**Approved design — dependency-inversion via closure injection** (the lb-l7↔lb-quic cycle means
`lb-quic` CANNOT import the relay `lb_l7::ws_proxy::proxy_frames`; only the `lb` binary sees both):
1. Inject `ws_relay_launcher: Arc<dyn Fn(H3WsTunnel, WsConnectReq) -> JoinHandle + Send + Sync>` from
   `lb` main → `QuicListenerParams` → `RouterParams` → `ActorParams` (mirror the EXISTING
   `config_factory: Arc<dyn Fn()->quiche::Config>` already threaded through that chain). The closure
   (in `lb`, sees both crates) dials the H1 backend + completes the upstream RFC 6455 handshake
   **INLINE BEFORE the 200** (the H3 analog of F-S27-1 / the H1 GHSA — correct ordering from the
   start), then runs `lb_l7::ws_proxy::proxy_frames` over the tunnel (R12 single-source kept).
2. `conn_actor::poll_h3`: on a validated extended CONNECT (validator already accepts it under
   ws_enabled), build `H3WsTunnel` + `H3TunnelEndpoints`, call the launcher, send `send_response(200)`
   ONLY after the launcher signals upstream-ready (else RESET/502-504). Then run that sid in
   "tunnel mode" each tick: `stream_recv → to_reader.try_send(Data)` (full ⇒ stop reading = QUIC
   flow-control backpressure, R8); `from_writer.try_recv → stream_send`; client FIN → drop inbound
   sender (clean EOF → tungstenite Close); peer RESET / Finished-on-reset / STOP_SENDING →
   `to_reader.try_send(Reset)` (reset-vs-clean-EOF, the F-MD-4-adjacent guard).
3. Real-wire e2e (`crates/lb-quic/tests/ws_h3_e2e.rs`): a `quiche::h3` client extended CONNECT through
   the binary's `spawn_quic` (F-S26-1 already wires H3→H1) to a tungstenite H1 WS echo backend; assert
   bidirectional frame relay + close + reset mapping + R8 backpressure + R13 burst + RFC 9220
   conformance. Reference harness: `crates/lb-quic/tests/h3_h1_stream_body_e2e.rs` (real quiche client
   + real QuicListener.with_backends) + builder-1's F-S26-1 test
   (`spawn_quic_h3_terminate_forwards_to_h1_backend_through_real_listener`).
Risks (from the A/B build): the actor tunnel-mode pump (multiplexing a long-lived tunnel stream in the
sync poll loop) + the upstream-before-200 ordering state machine are the hard parts; launcher threading
is mechanical (config_factory precedent). Building blocks ALREADY landed: validator (Stage A),
`ws_tunnel.rs` adapter (Stage B), F-S26-1 binary wiring, `WebsocketConfig.h3_extended_connect` flag
(opt-in; a NEWNESS gate, NOT a DoS gate — H3 backpressures by construction; doc says so).
Add an H3 WS soak scenario (sc8c) once the relay lands.

## 2. CF-S27-2 — WS-over-H2 true backpressure (the genuinely-large carry)
WS-over-H2 is shipped GATED OFF-by-default (`WebsocketConfig.h2_extended_connect`, default false)
because the relay's H2 client-write leg buffers UNBOUNDED when the client stalls: hyper's
`UpgradedSendStreamTask` (hyper-1.9.0 `upgrade.rs:98,119`) calls `h2::SendStream::send_data` even on
`Poll::Pending` capacity → h2's unbounded send buffer; `max_send_buf_size` only caps reported
`capacity()`. The true fix needs window-gated writes on the raw `h2::SendStream`
(`reserve_capacity`/`poll_capacity`), but hyper's `Upgraded` (`H2Upgraded`, `pub(super)`) doesn't
expose it → either drive raw h2 for WS streams (large; touches the mature H2 serving path, R3-risky) or
an upstream hyper enhancement. Reconciliation: `audit/websockets/s27-fs27-2-recon/F-S27-2-resolution.md`.
WS-over-H1 is SAFE (TCP socket WouldBlock backpressures); H3 SAFE by construction.

## 3. gRPC-over-H3 (the program's last spec item, ~1-2 sessions per the S27 prompt)
gRPC conformance over the migrated quiche::h3 stack. Not started.

## 4. Other carry-forwards (unchanged from S26/S27)
- CF-QUICHE-UPGRADE (transport #1-10 + QPACK uni-stream #23/#25 + §7.1 frame-completeness — own workstream).
- CF-DEP-1 (Dependabot — owner; GitHub flags 2 vulns/moderate+low on every push; the single oldest program item).
- CF-FCAP1-FLAKE (pre-existing H2-timeout race, isolation-proven).
- F-ESC-1 (multi-kernel CI lane), N-1 (jumbo-MTU), Mode A deferred perf tiers, CF-S15-PASSTHROUGH-RETRY-ODCID.
- LOW (S27, dispositioned, not carried): F-S27-3 trace-parity FIXED; lenient :scheme/:path FIXED.

## 5. LOW residuals surfaced in S27 (tracked, non-blocking)
- **close_backpressure teardown can block on a wedged sink (LOW).** `ws_proxy.rs::close_backpressure`
  (the F-S27-2 anti-hang write-timeout guard) itself does an un-`timeout`-wrapped
  `backend_tx.send(Close(None)).await` during teardown, so if the wedged side never drains, the
  teardown send can block. Bounded in practice by the connection/idle timeout; the wedged side is
  already pathological. Surfaced by the coverage test `close_backpressure_1008_on_forward_write_timeout`
  (which had to add a delayed drainer to complete teardown). Consider wrapping the teardown sends in a
  small budget. NOT in S27 scope.
- **Soak harness (LOW, verifier-p3):** (a) `loadgen` `Some(Err(_))→Closed` reclassifies a transport
  read-error to a benign reconnect — a defect surfacing purely as a transport Err (not wrong-bytes /
  not clean-Close) wouldn't increment the soak `err` count (it would still show as a cratered ok-rate /
  fd-RSS anomaly; doesn't affect the leak-class target). (b) a stale `eg-soak.rs:681-683` comment says
  `accept_inflight` is "scraped for visibility" but `ws_gauges()` omits it (cosmetic). Tidy in S28's
  soak work (the H3 soak scenario).
