# SESSION 29 — handoff (from S28, WS-over-H3 COMPLETE)

S28 completed **WS-over-H3 (RFC 9220)** — the last WebSocket transport. WS is now
proxied over **H1 (RFC 6455)**, **H2 (RFC 8441, gated)**, and **H3 (RFC 9220,
gated as a newness gate)**. Stage C (the carried relay wiring) is done +
verified end-to-end. S29 takes the program's LAST spec item: **gRPC-over-H3**.
Base: main after the S28 promote (confirm tip first).

## 1. gRPC-over-H3 (the program's LAST spec item; top priority)
gRPC conformance over the migrated `quiche::h3` stack. gRPC is HTTP/2-framed by
spec, but gRPC-over-H3 (the natural extension on a QUIC/H3 LB) carries the same
trailers-terminated, `application/grpc` content-type, `grpc-status`/`grpc-message`
trailer semantics over H3. The H3-terminate front already does H3 trailers
(RFC 9114 §4.1, `h3_bridge` request/response trailing field sections) and
content-type-agnostic relay, so the work is conformance + a real-wire gRPC e2e:
  - Verify the H3→H1/H2/H3 cells preserve `grpc-status` / `grpc-message` trailers
    end-to-end (the trailers path is built; prove it for gRPC specifically).
  - `grpc-timeout` header → deadline propagation (the H2 path has
    `GrpcProxy`/`GrpcConfig` with `max_deadline`; check the H3 path).
  - A real-wire gRPC-over-H3 e2e (a tonic/grpc client → H3 LB → gRPC backend), or
    a hand-rolled HTTP/3 gRPC frame test if a tonic-H3 client is unavailable.
  - The existing `[listeners.grpc]` config block is wired on the H2 path
    (`build_h2_proxy → with_grpc`); decide whether the QUIC listener needs the
    analogous wiring or whether gRPC-over-H3 is just content-agnostic relay +
    trailer fidelity (likely the latter for v1).
~1 session to full spec per the S27/S28 program plan.

## 2. WS-over-H3 — what shipped (S28), and what is opt-in
- **Stage C complete**: the closure-injection relay launcher (dependency
  inversion, mirrors `config_factory`) threads `Option<WsRelayLauncher>` through
  `QuicListenerParams → RouterParams → ActorParams`; `conn_actor::pump_ws_tunnels`
  runs the bounded tunnel; `lb::build_ws_h3_launcher` dials the H1 backend +
  completes the upstream RFC 6455 handshake BEFORE the 200 (the H3 analog of the
  WS-H1 GHSA / WS-H2 F-S27-1 ordering), then runs the single-sourced
  `lb_l7::ws_proxy::proxy_frames` over the `H3WsTunnel` seam (R12).
- **OFF by default** (`websocket.h3_extended_connect`, default false) as a
  **newness gate** (NOT a DoS gate — H3 backpressures by construction, proven R8;
  distinct from WS-H2's CF-S27-2 backpressure caveat). Enable per-listener once it
  has proved out in production.
- Verified: real-binary e2e (echo roundtrip), R8 wired backpressure (backend
  plateaus under a stalled client + drains on resume), R13 burst (50 cycles) +
  reset-vs-EOF negative control, RFC 9220 conformance
  (`s28-rfc9220-conformance.md`), WS-H3 soak (sc8c_ws_h3, bounded fds/RSS,
  panic=0).
- **LOW residual (S28):** the conn_actor's `next_wait` is capped to 2 ms whenever
  a WS tunnel is live (nothing wakes the actor `select!` when the relay produces
  an OUTBOUND frame — the same proven body/resp streaming pattern). This is a
  bounded busy-poll per WS connection (idle tunnels are reaped by `proxy_frames`'
  60 s idle). It scales with concurrent WS-H3 connections; a future optimization
  is an explicit relay→actor wakeup (a `Notify` or merged `from_writer` select)
  so idle tunnels don't poll. NOT a correctness/leak issue (soak fds bounded);
  tracked as CF-S28-WSH3-WAKEUP. Consider when WS-H3 graduates from the newness gate.

## 3. CF-S27-2 — WS-over-H2 true backpressure (carried, own workstream)
Unchanged from S28. WS-over-H2 stays GATED OFF (`h2_extended_connect`) until the
window-aware backpressure rearchitecture lands (hyper `H2Upgraded` buffers
unbounded; needs raw `h2::SendStream` window-gated writes — large, touches the
mature H2 serving path). H3 does NOT have this problem (the `H3WsTunnel`
`PollSender` parks the relay + quiche's per-stream send buffer is window-capped).

## 4. Other carry-forwards (unchanged)
- CF-QUICHE-UPGRADE (transport #1-10 + QPACK uni-stream #23/#25 + §7.1 frame-completeness).
- CF-DEP-1 (Dependabot — owner; 2 vulns moderate+low flagged on every push; oldest program item).
- CF-FCAP1-FLAKE (pre-existing H2-timeout race, isolation-proven).
- F-ESC-1 (multi-kernel CI lane), N-1 (jumbo-MTU), Mode A perf tiers, CF-S15-PASSTHROUGH-RETRY-ODCID.
- CF-S28-WSH3-WAKEUP (the 2 ms busy-poll, §2 above).

## 5. Program status
WebSockets: H1 ✓ (shipped), H2 ✓ (gated), H3 ✓ (gated newness). After S29
(gRPC-over-H3) the protocol matrix is spec-complete on the migrated quiche::h3
stack.
