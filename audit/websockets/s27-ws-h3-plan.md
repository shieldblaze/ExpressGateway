# WS-over-HTTP/3 (RFC 9220) — Phase 2 design of record

Grounded architecture scout (read-only) over conn_actor.rs / h3_bridge.rs / h3_config.rs /
listener.rs. All claims file:line-cited below. **Headline: WS-over-H3 done CORRECTLY is genuinely
large** — the actor's `poll_h3` is synchronous, so deferring the H3 2xx until the upstream WS
handshake succeeds (mandatory — the owner required upstream-before-success from the start, to NOT
replicate F-S27-1) needs a NEW per-stream async-coordination state machine in the actor (RISK-2).

## Reused verbatim (R12)
- Relay core `WsProxy::proxy_frames` (ws_proxy.rs:217-355); `server_ws`/`client_ws` wrap any
  `AsyncRead+AsyncWrite+Unpin` IO (ws_proxy.rs:370-387). Client side is protocol-agnostic.
- Backend leg = H2 path's dial verbatim: `pool.acquire_async(addr)` → `take_stream()` →
  `tokio_tungstenite::client_async_with_config(...)` → `backend_ws` (h2_proxy.rs:1340-1366 post-fix).
  H3 listener already dials H1 via the same TcpPool (h3_bridge.rs:1638-1675); backend addr from
  `select_backend(&params.backends)` (conn_actor.rs:1108,1261) — populated by `with_backends`
  (post-F-S26-1).
- quiche: `h3::Config::enable_extended_connect(true)` advertises SETTINGS_ENABLE_CONNECT_PROTOCOL
  (quiche-0.28 h3/mod.rs:614-651). quiche does NO pseudo-header validation — it passes the raw list
  to `Event::Headers{list, more_frames}` (mod.rs:2960-3005, doc 758-759) → `:protocol` is surfaced
  verbatim; app-layer is the sole enforcement point.

## H3 request lifecycle (conn_actor.rs)
`run_actor` (185-350) owns quiche `Connection` + `h3::Connection` in ONE task. `poll_h3` (950-1236):
PASS1 capacity-gated `drain_request_body` (the R8 read-gate, 967-977 / 871-930: only `recv_body`
while `tx.capacity()>0`); PASS2 `h3.poll` one event/call. Initial HEADERS arm (982-1158):
`validate_request_pseudo_headers` (1019) → `H3Request::from_headers` (1028) → :authority check
(1032-1063) → `bodyless = !more_frames` (1065) → dispatch to h3_to_h1/h2/h3_stream_resp
(1073-1128). Response write-back: `drain_streams_to_conn` (582-739) Progressive arm →
`send_response` (632) / `send_body` (647, partial-write tail retain 652-656) / FIN `send_body(&[],true)`
(716) / abort `stream_shutdown(Write, H3_INTERNAL_ERROR)` (703). Reset-vs-EOF: `Event::Finished`
re-probes `stream_recv` → StreamReset⇒Reset else End (the F-MD-4 "Finished-on-reset smuggle guard",
1187-1203); `Event::Reset` (1207).

## Stages
- **Stage A (config + validator; R3-safe, no behavior change when WS off)** — TRACTABLE.
  Thread `WsConfig`/`ws_enabled` QuicListenerParams→RouterParams→ActorParams; gate
  `enable_extended_connect(true)` in `build_server_h3_config` (h3_config.rs:50-58) on WS-enabled.
  enable_extended_connect adds an H3 SETTINGS entry (NOT a transport param — listener.rs:426-456
  untouched) → R3-safe at transport. Extend `validate_request_pseudo_headers` (h3_bridge.rs:469-561)
  to recognize `:protocol` ONLY under WS-enabled (else preserve today's reject at 512-514, R3).
  **RISK-1 (HIGH/spec): extended CONNECT pseudo-header shape.** RFC 8441 §4 / RFC 9220 §3: extended
  CONNECT carries `:method=CONNECT` + `:protocol` + `:scheme` + `:path` + `:authority` — UNLIKE classic
  CONNECT which OMITS :scheme/:path. The validator CONNECT arm (534-542) currently FORBIDS
  :scheme/:path. Must split classic-CONNECT (no scheme/path) from extended-CONNECT (scheme+path
  REQUIRED). Verify against RFC text before coding; security-sensitive (must not weaken classic guard).
- **Stage B (intercept + tunnel adapter)** — MODERATE.
  Detect at poll_h3 after validation, BEFORE cell dispatch (1065) [single choke point, R12]: headers
  contain `:method==CONNECT` && `:protocol==websocket` (ci). Adapter = two bounded mpsc channels +
  an AsyncRead/AsyncWrite newtype (mirror tests/ws_h2_e2e.rs H2StreamIo:210-313): inbound reuses the
  body-channel read-gate (R8 backpressure verbatim); outbound a bounded mpsc → new StreamTx variant
  drained by drain_streams_to_conn `send_body`. ~4 new per-stream HashMaps in run_actor. Per-tick
  wait cap (260-262) must include active tunnels.
- **Stage C (async dial coordination — THE HARD PART, RISK-2, R6-escalation candidate)**.
  poll_h3 is SYNC and cannot `.await` the dial+handshake. Correct (no-F-S27-1) design: spawn the dial
  task; hold the CONNECT stream "pending-upstream"; the task signals success/refuse/timeout back to
  the actor over a channel; actor emits `send_response 2xx`(fin=false) on success or RESET/5xx on
  failure. This is NEW per-stream async-coordination state that does not cleanly reuse the RespEvent
  flow (the head is conditional on an async outcome). Largest new mechanism; most likely scope blow-up.
- **Stage D (teardown)**: clean client FIN → drop inbound sender → adapter EOF → tungstenite clean
  Close. Client RESET / Finished-on-reset / peer STOP_SENDING (reap_client_cancelled_responses
  365-392) → adapter ABORT → backend reset (never a clean Close = smuggle/desync guard). Backend
  clean Close → outbound drop → `send_body(&[],true)`; backend error → `stream_shutdown` abort.
- **Stage E (tests)**: new `crates/lb-quic/tests/ws_h3_e2e.rs`. Real-wire: client
  `quiche::h3::Connection` w/ enable_extended_connect, `send_request([:method=CONNECT,:protocol=websocket,
  :scheme,:path,:authority], fin=false)` (quiche encodes arbitrary pseudo-headers w/o validation,
  mod.rs:1111-1139), then WS frames as H3 DATA via send_body/recv_body, client-side tungstenite over
  a mirror adapter. Reference harness: crates/lb-quic/tests/h3_h1_stream_body_e2e.rs (real quiche
  client + real QuicListener.with_backends + TCP backend). Tests: happy round-trip; reset-vs-EOF
  (client RESET mid-frame → backend abort, never clean Close); R8 backpressure (stalled reader bounds
  in-flight).

## Risks ranked
1. RISK-1 (HIGH/spec) — extended-CONNECT pseudo-header set (scheme/path REQUIRED). Verify RFC.
2. RISK-2 (HIGH/arch, R6 candidate) — async dial coordination from the sync actor; new state machine.
3. RISK-3 (MED) — net-new AsyncRead/AsyncWrite adapter + channel plumbing in lb-quic.
4. RISK-4 (MED) — never-FIN tunnel: empty `recv_body` must NOT be read as EOF; teardown only via
   Finished/Reset/STOP_SENDING.
5–7 (LOW) — WsConfig threading; per-tick cap; tunnel FIN ordering vs Progressive head.

## Sequencing decision
Build Stage A (R3-safe, tractable) + Stage B, then **owner/lead check-in before Stage C** (the
async-coordination state machine — the genuinely-large piece). If Stage C proves too large for the
budget, honest-stop per exit (b): land WS-over-H2 verified + carry WS-over-H3 with precise state.
