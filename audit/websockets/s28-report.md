# SESSION 28 — WS-over-H3 (RFC 9220) Stage C: report

Base: `main @ 5267147c` (S27 PARTIAL promoted). Branch:
`feature/ws-h3-stagec-s28`. Goal: finish WS-over-H3 (the carried Stage C),
verify end-to-end on the real binary, prove RFC 9220 conformance + R8 +
R13 + reset-vs-EOF on the wired tunnel, run a WS-over-H3 soak, promote.

WS-over-H3 stays **opt-in** (`websocket.h3_extended_connect`, default
false) as a **newness gate** (NOT a DoS gate — H3 backpressures by
construction, unlike WS-H2/CF-S27-2).

---

## Phase 0 — baseline + hygiene

- Base tip confirmed `5267147c`; branch `feature/ws-h3-stagec-s28` created
  + pushed to origin.
- `ps aux`: no S27 strays (no cargo/rustc/soak procs at start).
- Cores 8, RAM 15Gi (11Gi free), ENA `ens5` UP.
- Disk at start: 43G free. (After the cold all-features build for the gate:
  ~21G free, eg-target 27G — surgical cleanup scheduled post-baseline,
  CF-DISK-1.)
- Foundation present (read + confirmed):
  - Stage A: `h3_config::build_server_h3_config(ws_enabled)` →
    `enable_extended_connect`; `h3_bridge::validate_request_pseudo_headers`
    accepts RFC 8441/9220 extended CONNECT under `ws_enabled`.
  - Stage B: `lb-quic/src/ws_tunnel.rs` — `H3WsTunnel` (bounded
    AsyncRead+AsyncWrite) + `H3TunnelEndpoints` + `TunnelInbound{Data,Reset}`,
    R8-in-isolation + reset-vs-EOF mapping (8 unit tests).
  - F-S26-1 binary wiring: `spawn_quic` + `wire_h3_terminate_backends`
    resolve `[[listeners.backends]]` → `with_backends` (H3→H1 leg).
- Binding ×3 gate (verifier-independent, `--all-targets --all-features
  -D warnings` + fmt + ×3 `--workspace --all-features`): **<pending>**.

---

## Stage C design (APPROVED S27 — dependency inversion via closure injection)

### The cycle and the seam
`lb-l7 → lb-quic` (lb-l7 depends on lb-quic), so `lb-quic` CANNOT import
`lb_l7::ws_proxy::proxy_frames`. Only the `lb` binary sees both crates.
So the relay is injected as a closure, mirroring the existing
`config_factory: Arc<dyn Fn()->Result<quiche::Config,_>>` already threaded
`QuicListenerParams`→(built in `QuicListener::spawn`)→`RouterParams`→
`ActorParams`.

### New types (in `lb-quic/src/ws_tunnel.rs`)
```
pub enum WsUpstreamOutcome {            // launcher → actor readiness
    Ready  { headers: Vec<(String,String)> },  // send 200 + tunnel
    Failed { status: u16 },                     // send 502/504, teardown
}
pub struct WsConnectRequest {           // actor → launcher target ctx
    pub authority: String,
    pub path: String,
    pub subprotocols: Option<String>,   // client sec-websocket-protocol
}
pub struct WsRelayHandle {              // launcher → actor
    pub ready: oneshot::Receiver<WsUpstreamOutcome>,
    pub task:  JoinHandle<()>,
}
pub type WsRelayLauncher =
    Arc<dyn Fn(H3WsTunnel, WsConnectRequest) -> WsRelayHandle + Send + Sync>;
```

### Threading (Phase 1 — mechanical, mirrors config_factory/ws_enabled)
`QuicListenerParams.ws_relay_launcher: Option<WsRelayLauncher>`
(+ `with_ws_relay_launcher`) → `RouterParams.ws_relay_launcher` (cloned in
`QuicListener::spawn`) → `ActorParams.ws_relay_launcher` (cloned per actor
in `router::spawn_new_connection`). Default `None` ⇒ byte-identical to S27
(R3 off=no-op). `Option<Arc<..>>` is `Clone`.

### The launcher closure (in `lb` binary, built in `wire_h3_terminate_backends`
H1 arm when `params.ws_enabled`)
Captures: resolved H1 backend addrs + `TcpPool` + `WsConfig` (from
`[listeners.websocket]`). On call `(tunnel, req)`:
1. build `oneshot` + spawn task:
2. task: `pool.acquire_async(addr) → take_stream()`; build tungstenite
   `ClientRequestBuilder ws://addr{path}` + `with_sub_protocol(..)` for each
   forwarded subprotocol; `client_async_with_config(builder, stream, cfg)`
   under the header-timeout budget — **upstream RFC 6455 handshake BEFORE
   readiness** (H3 analog of WS-H1 GHSA / WS-H2 F-S27-1; R12 same ordering).
3. on success: read upstream resp `sec-websocket-protocol` (if any) →
   `ready.send(Ready{headers})`, then
   `proxy_frames(server_ws(tunnel,cfg), backend_ws)` — the single-sourced
   relay (R12; no duplicate).
4. on failure: `ready.send(Failed{status})` (502 refused / 504 timeout),
   drop tunnel.

### conn_actor tunnel-mode pump (Phase 2 — THE REAL RISK)
New per-actor map `ws_tunnels: HashMap<u64, WsTunnelState>`:
```
struct WsTunnelState {
    endpoints: H3TunnelEndpoints,        // to_reader: Sender<TunnelInbound>, from_writer: Receiver<Bytes>
    ready: Option<oneshot::Receiver<WsUpstreamOutcome>>,  // None once resolved
    phase: PendingUpstream | Active,
    out_pending: Option<Bytes>,          // unsent send_body tail (outbound R8 gate)
    inbound_open: bool,                   // to_reader still held (false ⇒ FIN/Reset relayed)
    resp_sent: bool,                      // 200/error send_response done
    fin_sent: bool,                       // we FINed the H3 stream outbound
    _task: JoinHandle<()>,               // relay; aborted on teardown
}
```
**Interception** — `poll_h3` `Event::Headers`, AFTER
`validate_request_pseudo_headers` passes: if `method==CONNECT` and the
headers carry `:protocol`:
  * `:protocol != websocket` → 501 (inline Progressive, "unsupported
    :protocol") — RFC 8441 unknown protocol.
  * launcher `None` (defensive; binary always pairs ws_enabled+launcher) →
    502.
  * else build `H3WsTunnel::new()` → `(tunnel, endpoints)`; build
    `WsConnectRequest{authority,path,subprotocols}` from the request; call
    launcher → `WsRelayHandle`; insert `WsTunnelState{PendingUpstream}`;
    `continue` (NOT a normal cell, NOT registered in body_tx_by_stream).
**Event routing for WS sids** — `Event::Data/Finished/Reset`: if sid in
`ws_tunnels`, route to the tunnel (inbound pump / FIN→drop to_reader /
Reset→`to_reader.try_send(Reset)`); else existing body path. (Normal path
already no-ops a sid absent from `body_tx_by_stream`, but explicit routing
keeps it unambiguous + lets us re-arm inbound.)
**`pump_ws_tunnels(conn, h3, &mut ws_tunnels)`** each tick (after poll_h3):
  * `PendingUpstream`: `ready.try_recv()`:
    - `Ready{headers}` → `send_response(conn,sid,[:status 200]+headers,false)`;
      `resp_sent=true`; `phase=Active`. (If send_response blocks/Done, retry
      next tick — keep PendingUpstream-with-Ready-cached.)
    - `Failed{status}` → inline error response (Progressive 502/504) +
      teardown.
    - Empty → nothing.
  * `Active`:
    - **inbound** (client→backend): while `to_reader` has capacity AND no
      EOF/Reset latched: `h3.recv_body(conn,sid,scratch)` →
      `to_reader.try_send(Data(chunk))`. `Done`/0 ⇒ stop. Channel full ⇒
      stop reading (QUIC flow-control backpressure — R8 inbound). Error ⇒
      `to_reader.try_send(Reset)` + close inbound (F-MD-4-adjacent).
    - **outbound** (backend→client): if `out_pending` empty,
      `from_writer.try_recv()`; on a chunk set `out_pending`. Then
      `h3.send_body(conn,sid,&out_pending,false)`: partial ⇒ retain tail
      (R8 outbound — quiche send window full ⇒ from_writer not drained ⇒
      PollSender parks the relay writer ⇒ proxy_frames parks ⇒ backend read
      pauses). `from_writer` closed (relay finished) ⇒ `send_body(.., true)`
      FIN + `fin_sent`.
**Close/reset mapping (R13 / RFC 9220):**
  * client FIN (`Event::Finished`, NOT reset) → drop `to_reader` ⇒ tunnel
    reader EOF ⇒ `proxy_frames` `client_rx None` ⇒ Close(None) to backend.
  * client Reset / Finished-on-reset (probe `conn.stream_recv(sid,&mut[])`
    → `StreamReset`) / STOP_SENDING → `to_reader.try_send(Reset)` ⇒
    `ConnectionReset` ⇒ `proxy_frames` Err ⇒ teardown (NOT a clean End).
  * relay finished (from_writer closed) → FIN the H3 stream outbound.
  * teardown: drop `to_reader`, `out_pending`, abort `_task`, remove state.
**`next_wait` tick:** include `!ws_tunnels.is_empty()` in the 2ms-cap
condition so a backpressured/active tunnel re-polls promptly (same S2/S4
reasoning; does NOT defeat the bounded channels).

### R8 on the wired tunnel (re-proven in Phase 3, not assumed from Stage B)
Bounded both directions, message-volume-independent:
  * inbound retained ≤ `H3_WS_TUNNEL_DEPTH * H3_WS_TUNNEL_CHUNK_MAX` (≈64KiB)
    + one scratch chunk; over-cap stays in quiche's flow-control buffer.
  * outbound retained ≤ depth*chunk + `out_pending` (≤ chunk).

---

## Phase 1 — launcher threading (off=no-op proof)
*(pending)*

## Phase 2 — tunnel-mode pump (R8 wired proof, reset-vs-EOF)
*(pending)*

## Phase 3 — e2e + RFC 9220 conformance + R13 + soak + promote
*(pending)*

## Verdict
*(pending)*
