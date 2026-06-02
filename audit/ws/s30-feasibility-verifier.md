# S30 — WS-over-H2 window-aware backpressure: INDEPENDENT feasibility verdict

Verifier: independent read of primary sources (vendored hyper 1.9.0 + 1.10.1,
h2 0.4.13, futures-channel 0.3.31) and the in-repo relay/handler. I did NOT
assume the F-S27-2 diagnosis; I re-derived the data path from source.

Question under test: can the WS-over-H2 tunnel obtain **window-aware**
backpressure (stalled consumer ⇒ bounded gateway memory) via a CONTAINED change
that does NOT alter how hyper serves GENERAL (non-CONNECT) H2 requests, via
"window-gated writes on the raw `h2::SendStream`, bypassing hyper's `Upgraded`
for the WS-tunnel path ONLY"?

---

## The actual data path (re-derived from source)

The relay `WsProxy::proxy_frames` (crates/lb-l7/src/ws_proxy.rs:392) writes a
client-bound frame with `client_tx.send(msg).await`. `client_tx` is a
tungstenite `WebSocketStream` wrapping `TokioIo::new(upgraded)`, where `upgraded`
is hyper's `Upgraded` obtained from `hyper::upgrade::on(&mut req)`
(h2_proxy.rs:1513-1524). On flush, tungstenite calls
`Upgraded::poll_write` → `H2Upgraded::poll_write`.

`H2Upgraded` (hyper-1.9.0/src/proto/h2/upgrade.rs:39-44) does NOT hold the
`h2::SendStream`. It holds an `mpsc::Sender<Cursor<Box<[u8]>>>`
(`UpgradedSendStreamBridge`, line 46-49). The real `h2::SendStream` lives in a
SEPARATE detached future, `UpgradedSendStreamTask` (line 53-59), spawned by
hyper at upgrade time (`self.exec.execute_upgrade(up_task)`,
server.rs:503). The two halves are joined by `mpsc::channel(1)`
(upgrade.rs:21). So the byte path is:

  tungstenite → `H2Upgraded::poll_write` → `mpsc(1)` → `UpgradedSendStreamTask::tick` → `h2::SendStream::send_data`

This split is the crux of every sub-question below.

---

## Q1 — ROOT CAUSE: is `send_data` called unconditionally regardless of window?

**YES.** `UpgradedSendStreamTask::tick` (upgrade.rs:68-133). The loop:

- line 79: `me.h2_tx.reserve_capacity(1);` — reserves only 1 byte.
- lines 81-101: if `capacity()==0`, loop on `poll_capacity`. The relevant arm:
  ```
  98:  Poll::Pending => break 'capacity,
  ```
  On no window, `poll_capacity` is `Pending` ⇒ `break 'capacity` breaks ONLY
  the inner capacity loop, then execution FALLS THROUGH (it does NOT return
  Pending here).
- lines 103-114: `poll_reset` check (Pending ⇒ `()`), falls through.
- lines 116-131: `me.rx.as_mut().poll_next(cx)`:
  ```
  116:  match me.rx.as_mut().poll_next(cx) {
  117:      Poll::Ready(Some(cursor)) => {
  118:          me.h2_tx
  119:              .send_data(SendBuf::Cursor(cursor), false)   // <-- UNCONDITIONAL
  120:              .map_err(crate::Error::new_body_write)?;
  121:      }
  ```
  If the channel has a queued cursor, `send_data` is called **with no regard to
  whether `capacity()` is 0**. The function then loops back to the top: reserve
  1, capacity still 0, `poll_capacity` Pending ⇒ `break`, drain rx again, …

  `tick` only returns `Poll::Pending` (line 128-129) when the **mpsc channel is
  empty** — i.e. when the producer momentarily has nothing queued. It never
  returns Pending because the *window* is closed. Therefore a stalled consumer
  (zero `WINDOW_UPDATE`) does NOT stop `tick` from pulling every frame the
  producer offers and pushing it into `send_data`.

Verdict Q1: confirmed. `send_data` is called unconditionally; `poll_capacity`
Pending is swallowed (`break 'capacity`), not propagated as backpressure.

## Q2 — does `h2::SendStream::send_data` bound its buffering with no window?

**NO — explicitly unbounded, by the crate's own docs and impl.**

Docs (h2-0.4.13/src/share.rs):
- lines 52-54: "If the caller attempts to send data on a stream when there is
  no available window capacity, the library will buffer the data until capacity
  becomes available …"
- lines 56-59: "**NOTE**: There is no bound on the amount of data that the
  library will buffer. If you are sending large amounts of data, you really
  should hook into the flow control lifecycle. Otherwise, you risk using up
  significant amounts of memory."
- lines 326-331 (`send_data` rustdoc): "this buffering is unbounded."

Impl (h2-0.4.13/src/proto/streams/prioritize.rs `Prioritize::send_data`):
- line 173: `stream.buffered_send_data += sz as usize;` — counter only, no cap.
- lines 210-219: when `stream.send_flow.available() == 0` (closed window) and
  the frame is non-empty:
  ```
  218:  stream.pending_send.push_back(buffer, frame.into());
  ```
  The frame is pushed onto an unbounded per-stream `pending_send` queue
  (field declared prioritize.rs:28) and the connection task is NOT notified.
  There is no length/byte ceiling anywhere on this path.

What is buffered is a `SendBuf::Cursor(Cursor<Box<[u8]>>)`
(hyper-1.9.0/src/proto/h2/mod.rs:223-225) — a **heap-owned copy** of each WS
frame. So each stalled-consumer frame becomes a retained `Box<[u8]>` in
`pending_send`, growing without limit.

Note: hyper's `max_send_buffer_size` (server.rs:39, default 400 KiB) is passed
to `h2::Builder::max_send_buffer_size` (server.rs:138) and is consulted in
`try_assign_capacity` (prioritize.rs:466 `assign_capacity(assign,
self.max_buffer_size)`) to cap how much *assigned window capacity* a stream may
hold — it does NOT cap `pending_send` for frames queued while the window is
zero. It throttles `poll_capacity` grants, not `send_data` buffering. So it
does not save the upgrade path.

Verdict Q2: confirmed unbounded. The DoS lives in h2's `pending_send`, fed by
the unconditional `send_data` in Q1.

## Q3 — SEAM: can the WS handler get the raw `h2::SendStream` / the window for
JUST the extended-CONNECT stream while hyper serves all other streams?

**NO. Every door is closed by hyper's encapsulation.**

(a) `Upgraded::downcast` — is the inner H2 type nameable?
- `H2Upgraded` is `pub(super)` (upgrade.rs:39 `pub(super) struct H2Upgraded`).
  It is NOT exported from hyper's public API. `Upgraded::downcast::<T>()`
  (upgrade.rs:151) requires naming `T`; you cannot name `H2Upgraded` from
  outside the crate, so you cannot downcast to it.
- Even if you could downcast to it, `H2Upgraded` holds only the
  `UpgradedSendStreamBridge` (the `mpsc::Sender` end) and a `RecvStream`
  (upgrade.rs:40-43). The `h2::SendStream` itself was MOVED into
  `UpgradedSendStreamTask` (upgrade.rs:32-34) which hyper spawned and which we
  cannot reach. So downcast — even if it compiled — yields the channel sender,
  NOT the window-bearing `SendStream`. The seam the prompt names ("raw
  `h2::SendStream`") is **not present in the upgraded object at all**.

(b) Does hyper deliver the CONNECT body as a normal flow-controlled body, or
capture it into `connect_parts`?
- server.rs:272-299: on `is_connect`, hyper does NOT build an `IncomingBody`
  from the stream. It builds `IncomingBody::empty()` (line 292) and stashes the
  `RecvStream` into `ConnectParts { recv_stream: stream, … }` (line 296),
  plus inserts an `OnUpgrade` extension. The receive half is therefore reachable
  only via the `Upgraded` (as `H2Upgraded.recv_stream`), again private. There is
  no public handle to the CONNECT stream's `SendResponse`/`SendStream`.

(c) Can a CONNECT response stream a normal flow-controlled response body
instead of upgrading?
- server.rs:478-505: when `connect_parts` is `Some` AND the response status
  `is_success()`, hyper UNCONDITIONALLY takes the upgrade fork: `reply!` sends
  the response (line 494), then `super::upgrade::pair(...)` (line 495) +
  `execute_upgrade(up_task)` (line 503) + `return Poll::Ready(Ok(()))` (line
  504). The `H2Stream::poll2` returns immediately — it NEVER reaches the
  `PipeToSendStream` body path (lines 508-520) for a successful CONNECT. So you
  cannot ask hyper to "stream a normal flow-controlled body on the CONNECT
  stream": a 2xx to a CONNECT is hardwired to the upgrade machinery. (A non-2xx
  CONNECT skips the upgrade and is a normal response — but that is a refusal,
  not a tunnel.) There is no API switch to keep the CONNECT stream as a plain
  flow-controlled `SendStream` you own.

Verdict Q3: no seam. The window-aware object (`h2::SendStream`) is consumed by a
hyper-private detached task; nothing public on `Upgraded`, the request, or the
response lets the WS handler reach it or substitute a flow-controlled body.

## Q4 — does raw `h2::SendStream` access require driving `h2::server` directly,
and does that force re-implementing GENERAL H2 serving for that connection?

**YES to both, by the nature of H2 multiplexing.**

- The only way to get a raw, window-bearing `h2::SendStream` per stream is to
  own the `h2::server::Connection` yourself (`h2::server::handshake(io)` →
  `conn.poll_accept()` → `(Request, SendResponse)`; `SendResponse::send_response`
  returns the `SendStream`). hyper does not re-expose this for an established
  connection; `Upgraded` is the only handle it hands back.
- H2 multiplexes ALL streams (regular GET/POST requests AND the extended
  CONNECT) over ONE `h2::server::Connection`. A single `poll_accept` loop
  yields every inbound stream on that connection. You cannot "peel out" only the
  CONNECT stream from a hyper-owned connection: hyper owns the `Connection`
  future (`Serving.conn`, server.rs:111) and the accept loop
  (`poll_server`/`poll_accept`, server.rs:259). There is no API to extract one
  stream's `SendStream` while leaving the rest under hyper. The `SendStream` for
  a stream is minted inside hyper's accept loop and (for CONNECT) immediately
  buried in the private upgrade task.
- Therefore, to get window-aware writes you must run `h2::server` for the WHOLE
  connection yourself, which means re-implementing hyper's general request
  serving (routing, body flow-control, header strip/X-Forwarded/Via,
  H2→{H1,H2,H3} dispatch, the entire `H2Proxy::handle_inner` machinery,
  date-header, CONNECT-body rejection, RST handling, GOAWAY/glitches/CleanCloseIo
  teardown) for every NON-CONNECT stream on that connection. That is precisely
  the "alter how hyper serves general H2 requests" the question forbids — except
  worse: it removes hyper from the path entirely for those connections.

Verdict Q4: a raw-SendStream seam cannot be contained to the CONNECT stream;
it forces re-hosting the whole connection on bare `h2::server`.

## Q5 — HYPER FORK: would patching `UpgradedSendStreamTask::tick` to gate
`send_data` on real capacity be small, and would it touch general H2 serving?

**Small patch; surgically isolated to the upgrade path; does NOT touch general
H2 serving — but it is a dependency-governance decision, not an in-repo fix.**

- The minimal change is to make `tick` NOT pull from `rx`/call `send_data` until
  real capacity exists. Concretely: in upgrade.rs, when `capacity() == 0` and
  `poll_capacity` returns `Pending`, return `Poll::Pending` (and only drain `rx`
  when `capacity() > 0`, sending at most `capacity()` bytes). That converts the
  unbounded `pending_send` accumulation into true backpressure: the `mpsc(1)`
  fills, `H2Upgraded::poll_write`'s `tx.poll_ready` (upgrade.rs:205) returns
  Pending, tungstenite's sink parks, `proxy_frames`'s
  `client_tx.send().await` parks — bounded memory (≈ 1 frame in the channel +
  ≤ window in h2). This is a ~10-20 line change confined to `tick`.
- Blast radius: `UpgradedSendStreamTask` / `H2Upgraded` are used ONLY for H2
  (and the same module shape for upgraded CONNECT) — i.e. the upgrade path. The
  general request path uses `PipeToSendStream` (server.rs:516,
  H2StreamState::Body), a DIFFERENT type that already reserves capacity
  correctly. So patching `tick` does NOT change how hyper serves non-upgrade
  streams. (`pair`/`H2Upgraded` are `pub(super)`, only reachable via the upgrade
  fork.)
- BUT this is editing a third-party crate. Options: a `[patch.crates-io]`
  vendored hyper fork, or upstreaming to hyperium/hyper. It is NOT "an in-repo
  code change to ExpressGateway" — it is a forked-dependency you must carry and
  re-audit on every hyper bump. Classify: **dependency-governance decision
  (carry a hyper fork)**, not a contained in-repo seam.
- The same defect is unfixed upstream as of the newest vendored hyper (Q6), so
  a fork would have to be maintained until/unless upstream fixes it.

## Q6 — is the upgrade path fixed in any newer hyper?

**NO.** `diff hyper-1.9.0 … hyper-1.10.1 src/proto/h2/upgrade.rs` shows the ONLY
change is cosmetic — `h2_to_io_error` swaps `.unwrap()` for `.expect("…")`
(upgrade.rs:276). `UpgradedSendStreamTask::tick` is byte-for-byte identical in
1.10.1: `reserve_capacity(1)`, `break 'capacity` on Pending, and the
unconditional `send_data` on `rx` drain are all unchanged. Upgrading hyper does
NOT fix this.

---

## FINAL VERDICT: **NOT-VIABLE** (no contained seam; the sanctioned direction
does not exist as described)

The sanctioned direction — "window-gated writes on the raw `h2::SendStream`,
bypassing hyper's `Upgraded` for the WS-tunnel path ONLY" — rests on a premise
that is false in source: for a hyper-served H2 connection there is **no raw
`h2::SendStream` reachable for the CONNECT stream**. hyper moves it into a
private, hyper-spawned `UpgradedSendStreamTask`; `Upgraded`/`downcast` expose
only an `mpsc::Sender` bridge (Q3a), the CONNECT recv half is captured into
private `ConnectParts` (Q3b), and a 2xx CONNECT is hardwired to the upgrade
machinery with no flow-controlled-body alternative (Q3c). The buffering is
unbounded by h2's explicit contract (Q2) and is fed by an unconditional
`send_data` that swallows `poll_capacity == Pending` (Q1).

The only ways to get window-aware writes are:

1. **Re-host the whole connection on bare `h2::server`** (drive
   `h2::server::handshake` + `poll_accept` yourself). Because H2 multiplexes
   regular requests and the CONNECT over one connection (Q4), this CANNOT be
   contained to the WS-tunnel path — it forces re-implementing general H2
   serving (and replacing hyper entirely) for any connection that might carry a
   WS tunnel. This is a **broad rearchitect**, the opposite of contained, and it
   directly alters how general H2 requests are served on those connections.

2. **Fork hyper** and gate `tick` on real capacity (Q5). This IS a small,
   surgically-isolated change that does NOT affect general H2 serving — but it
   is a **dependency-governance decision** (carry/maintain a `[patch.crates-io]`
   hyper fork, re-audited on every bump; unfixed upstream through 1.10.1 per
   Q6), not an in-repo ExpressGateway code change. The prompt scopes the
   question to a contained change in our code; a forked dependency is outside
   that scope.

Neither path is the contained, in-repo, raw-`SendStream` seam the question
posits. The current per-direction `read_frame` timeout in `proxy_frames`
(ws_proxy.rs:370-373, 392-395) does bound *time* (a wedged write is reclaimed in
≤ read_frame_timeout, default 30 s), so memory is bounded by
`producer_rate × 30 s` rather than truly window-gated — a mitigation, not the
asked-for true backpressure, and 30 s of a fast flooder is plenty for a memory
spike.

**Minimal escalation, in order of preference:**
- **Stay GATED OFF** (status quo, CF-S27-2) — zero risk, zero new surface; the
  capability remains opt-in and unadvertised. RECOMMENDED unless WS-over-H2 is a
  hard requirement.
- If WS-over-H2 must ship with true backpressure: **fork hyper** (`tick` gate,
  Q5) under explicit dependency governance — smallest correct change, isolated
  to the upgrade path, but an ongoing maintenance obligation. Prefer upstreaming
  the fix to hyperium/hyper so the fork can be retired.
- Do NOT pursue the bare-`h2::server` rehost as a "contained" fix — it is a
  broad rearchitect that removes hyper from the general H2 path.

Evidence cited is limited to files I read this session: hyper-1.9.0 &
hyper-1.10.1 `src/proto/h2/upgrade.rs`, `.../h2/server.rs`, `.../h2/mod.rs`,
`src/upgrade.rs`; h2-0.4.13 `src/share.rs`, `src/proto/streams/prioritize.rs`;
futures-channel-0.3.31 `src/mpsc/mod.rs`; and the repo's
`crates/lb-l7/src/{ws_proxy.rs,h2_proxy.rs}`.
