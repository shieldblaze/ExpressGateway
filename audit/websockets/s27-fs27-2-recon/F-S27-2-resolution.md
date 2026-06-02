# F-S27-2 contradiction resolution — independent recon

Neutral third reviewer (hyper/h2 internals). Resolved by independent
MEASUREMENT (producer-pushed-plateau gauge, NOT process RSS) + vendored
source reading. Crate versions from Cargo.lock: hyper 1.9.0, h2 0.4.13,
tungstenite / tokio-tungstenite 0.24.0.

Harness reused: the prior-engineer plateau test on disk
`tests/ws_r8_backpressure_plateau.rs` (committed in bd3f991d). It drives the
REAL `H1Proxy(with_websocket)` / `H2Proxy(with_websocket)` against a real
flooding WS backend (small write buffer + per-frame flush so the backend's own
`flush().await` parks on TCP backpressure once the gateway stops draining). The
gauge is the backend's pushed-message count at a non-reading client. No source
of the suite was modified or committed; a throwaway probe edit to
`crates/lb-l7/src/ws_proxy.rs` (revert the `max_write_buffer_size` bound to
tungstenite's `usize::MAX`) was made, measured, and REVERTED. Tree left clean.

FLOOD shape: 2048 messages x 256 KiB = ~512 MiB total. Plateau ceiling 256
messages (~64 MiB) — decisive vs the full flood, far above the true plateau.

---

## TASK 1 — Is WS-over-H1 safe? (headline)

VERDICT: **WS-over-H1 is BOUNDED / SAFE. F-S27-2 is NOT a live bug on the
shipped H1 WS path.** ws-eng's measurement is CONFIRMED; the verifier's
"identical on H1" claim is REFUTED.

Completed-run plateau numbers (each a finished test run, captured stdout):

| transport | `max_write_buffer_size` | backend pushed / 2048 |
|-----------|-------------------------|-----------------------|
| H1        | BOUND applied (committed tree) | **17 / 2048** (PASS) |
| H1        | BOUND reverted to `usize::MAX` | **17 / 2048** (PASS) |
| H1        | BOUND applied (final re-run)   | **17 / 2048** (PASS) |

(18/2048 on one earlier run — same plateau, scheduler jitter of one frame.)

The H1 gateway parks at ~17 messages (~4.3 MiB in-flight), NOT ~512 MiB. The
producer stalls because the relay's `client_tx.send().await` parks on the
client TCP socket's `WouldBlock` (tokio-tungstenite's `Sink` sets `ready=false`
in `start_send`, then `poll_ready`/`poll_flush` return `Pending`), which stops
the relay's single select-loop reading the backend, which fills the backend
socket and parks the backend's `flush().await`. End-to-end backpressure works.

Crucially, **H1 plateaus at the SAME value with the bound reverted to
`usize::MAX`** — so the `max_write_buffer_size` bound is NOT the lever; the raw
TCP socket already provides backpressure. The verifier's root-cause attribution
(`max_write_buffer_size = usize::MAX`) is the wrong lever; their suggested fix is
a no-op for backpressure on any `WouldBlock`-surfacing transport. ws-eng's
claim (d) is CONFIRMED.

---

## TASK 2 — H2 root cause (source-confirmed)

VERDICT: **CONFIRMED. WS-over-H2 is UNBOUNDED.** ws-eng's root-cause is correct
to file:line. The H2 absorbs almost the whole flood at a non-reading client:

| transport | `max_write_buffer_size` | backend pushed / 2048 |
|-----------|-------------------------|-----------------------|
| H2        | BOUND reverted to `usize::MAX` | **1472 / 2048** (~360 MiB) |
| H2        | BOUND applied (committed tree) | **1504 / 2048** (~368 MiB) |

The bound changes NOTHING on H2 either (1472 vs 1504 = jitter, both ~full
flood). No sign of a plateau — continued growth toward the flood = unbounded
buffering inside the gateway.

Source chain (vendored), each cited:

1. The relay forwards via `client_tx.send(msg).await` (ws_proxy.rs:392) where
   the client sink is tokio-tungstenite over hyper's `Upgraded`. For an upgraded
   H2 extended-CONNECT stream the concrete IO is hyper's `H2Upgraded`
   (`hyper-1.9.0/src/proto/h2/upgrade.rs:39`, `pub(super) struct H2Upgraded`).

2. `H2Upgraded::poll_write` (upgrade.rs:196-236) only gates on
   `self.send_stream.tx.poll_ready(cx)` — the `mpsc::channel(1)` sender
   (created at upgrade.rs:21, `mpsc::channel(1)`). On Ready it `start_send`s a
   `Cursor` into that channel and returns `Poll::Ready(Ok(n))` (upgrade.rs:222).
   It does NOT consult the HTTP/2 flow-control window.

3. The channel is drained by `UpgradedSendStreamTask::tick`
   (upgrade.rs:68-133). The capacity check at upgrade.rs:81-101 does
   `Poll::Pending => break 'capacity` (upgrade.rs:98) — i.e. when the window
   is exhausted it BREAKS the capacity loop and FALLS THROUGH to
   `me.rx.poll_next(cx)` (upgrade.rs:116) and `me.h2_tx.send_data(...)`
   (upgrade.rs:119) UNCONDITIONALLY. `tick` returns `Poll::Pending` only when
   the mpsc is empty (upgrade.rs:128-129). So the task always drains an
   available mpsc item into `send_data` regardless of window — the mpsc(1)
   never stays full → `poll_ready` essentially always returns Ready → the
   relay's `send().await` NEVER parks over H2.
   *** Verified IDENTICAL on hyper master (fetched 2026-06-01) — not a fixed
   bug; this is the current structural behavior. ***

4. `h2::SendStream::send_data` buffers unbounded when there is no window:
   - h2-0.4.13/src/share.rs:48-59 (doc): "If the caller attempts to send data
     on a stream when there is no available window capacity, the library will
     buffer the data... **NOTE**: There is no bound on the amount of data that
     the library will buffer."
   - h2-0.4.13/src/share.rs:319-338 (`send_data` doc): "this buffering is
     unbounded."
   - h2-0.4.13/src/proto/streams/prioritize.rs:145-219 (`send_data` impl):
     line 173 `stream.buffered_send_data += sz` is unconditional; with no window
     the frame is pushed onto `stream.pending_send` (line 218) — an unbounded
     `VecDeque`. There is NO `max_buffer_size` check on the send path.

5. hyper's `max_send_buf_size` (set by us to 64 KiB via
   `H2SecurityThresholds::apply`, h2_security.rs:88,117) does NOT bound the
   upgraded stream. It maps to h2's `max_send_buffer_size`
   (hyper server.rs:138 → h2 `prioritize.max_buffer_size`), which is used ONLY
   to compute the *reported* `capacity()` (h2 stream.rs:275-280:
   `available.min(max_buffer_size).saturating_sub(buffered)`). It does NOT
   reject or block `send_data`. And the upgraded `tick` bypasses the
   capacity-gating anyway (point 3). ws-eng's claim (c) is CONFIRMED.

ws-eng's claims (a), (b), (c) are all CONFIRMED at source. The verifier's
property (i) (volume-independent peak for a DRAINING consumer) is not in
dispute; only their root cause and "identical on H1" are wrong.

---

## TASK 3 — Fix feasibility (ranked)

### (A) hyper builder/feature knob to bound the upgraded H2 send buffer
DOES NOT EXIST / DOESN'T WORK. `max_send_buf_size` is already set (64 KiB) and
is structurally ineffective for the upgraded stream (Task 2 pts 3,5). The
unbounded `tick`→`send_data` path is present on hyper master. No public knob
bounds it. Effort: n/a. Verdict: DOESN'T.

### (B) Recover the raw `h2::SendStream` via `Upgraded::downcast`
NOT FEASIBLE through hyper's public API. `Upgraded::downcast::<T>()`
(hyper upgrade.rs:151) requires you to name the concrete inner type `T`, but
that type is `H2Upgraded`, declared `pub(super)` (upgrade.rs:39) — it is not a
public, nameable type. You cannot downcast to it from outside hyper, and there
is no accessor that yields the inner `h2::SendStream`/`RecvStream`. The only way
to obtain the raw `h2::SendStream` for window-gated `reserve_capacity` /
`poll_capacity` writes is to STOP using `hyper::server::conn::http2` for the WS
path and drive the `h2` crate directly for extended-CONNECT — a server/relay
rearchitecture for the H2 leg. Effort: LARGE (new h2-direct server accept path
+ extended-CONNECT handling + integration with the existing dispatch). Verdict:
DOESN'T (via hyper) / LARGE (via direct h2).

### (C) Relay-level bounded in-flight wrapper ("don't read next producer frame
until prior is DRAINED")
DOES NOT BOUND H2 — CRITICAL ASSUMPTION REFUTED. Over hyper-H2, `send().await`
(feed+flush on the tungstenite sink over `H2Upgraded`) returns as soon as the
bytes are buffered in h2, NOT after they leave the gateway. Proof: the relay
`proxy_frames` is ALREADY exactly this design — a single select-loop that does
not read the next producer frame until the prior `send().await` resolves
(ws_proxy.rs:279-400). Yet H2 absorbed 1504/2048 frames at a non-reading client.
"Drained" over H2 = "handed to h2's unbounded `send_data` buffer", which always
succeeds. A semaphore/byte-counter keyed off `send().await` completion would
release immediately on every H2 send and never throttle. So (C) cannot bound H2.
(It is, however, already correct for H1/raw where `send().await` genuinely parks
— which is why H1 is safe.) Verdict: DOESN'T (for H2).

### (D) Other mechanisms that actually bound H2
The only mechanisms that bound memory over hyper-H2 are ones that gate on the
H2 FLOW-CONTROL WINDOW rather than on `send().await` completion:
  - A reader-side byte budget keyed off ACKed window progress would require
    visibility into the upgraded stream's window — not exposed by `Upgraded`
    (same wall as B).
  - Driving the raw `h2::SendStream` with `reserve_capacity`/`poll_capacity`
    (= option B's rearchitecture).
  - Setting a small per-stream window does NOT help: `send_data` buffers above
    the window regardless (Task 2 pt 4) — the window throttles the WIRE, not
    the in-memory buffer; the gateway still absorbs the flood into RAM.
So D collapses into B. Verdict: requires the B rearchitecture.

### Recommended path + size
Genuinely-large for a TRUE H2 backpressure fix → **R6 ESCALATE**. There is no
tractable in-place fix: every option that actually bounds H2 memory requires
bypassing hyper's `Upgraded` and driving the raw `h2::SendStream` with
window-gated writes (option B), which is a relay/server rearchitecture for the
H2 extended-CONNECT leg, not a config/wrapper change. NOT tractable
this-session within a no-rearchitecture scope.

Pragmatic interim (tractable, already partly landed by ws-eng — KEEP): the
anti-hang `timeout(read_frame, send().await)` → Close 1008 guard does NOT bound
H2 in-flight memory (H2 `send` does not park, so the timeout never fires under
flood — it fires only on a transport where `send` CAN park, i.e. H1). For H2 the
only in-place mitigations are operational caps that BOUND the blast radius
without fixing the unbounded-per-stream behavior:
  - a small `WsConfig::max_message_size` (already a knob) caps a single in-flight
    frame but NOT the accumulation of many frames in h2's `pending_send`;
  - `max_concurrent_streams` caps the number of simultaneous tunnels;
  - neither bounds a single tunnel's RAM. The honest statement to the owner: the
    per-tunnel H2 unbounded-buffer DoS is not closeable without option B.

### Production-proxy context (one paragraph)
Envoy and nginx do NOT proxy WS-over-H2 through a high-level "upgraded socket"
abstraction. They own the HTTP/2 codec directly and manage per-stream send/recv
flow-control windows themselves: a stalled downstream stops sending
WINDOW_UPDATE, the proxy's stream send window goes to zero, the proxy stops
pulling from the upstream (it never copies bytes it has no window to forward),
and that read-pause propagates to the upstream as flow-control backpressure.
The key difference from hyper here is that they gate the COPY on window
availability (h2's `reserve_capacity`/`poll_capacity` discipline), whereas
hyper's `UpgradedSendStreamTask` copies into `send_data` unconditionally and
relies on h2's unbounded buffer — which is exactly the gap. This is the same
discipline option B would have to reimplement.

---

## TASK 4 — Does the planned WS-over-H3 inherit F-S27-2?

VERDICT: **NO — the planned H3 adapter backpressures BY CONSTRUCTION.** Reason
from the cited mechanism (conn_actor.rs), which is structurally the OPPOSITE of
hyper's `UpgradedSendStreamTask`:

OUTBOUND (gateway→client over the quiche stream):
  - "only refill an EMPTY queue" gate (conn_actor.rs:436-438:
    `if !queue.is_empty() { continue; }`) — the actor pulls exactly ONE response
    event per stream per tick and ONLY when the prior queue has fully drained to
    quiche.
  - partial-write tail retention (conn_actor.rs:649-656): a partial
    `send_body` `Ok(n)` with `n < b.len()` keeps the unsent tail at the front
    (`b.split_to(n)`) and breaks — does NOT force-drain. `Ok(0)` / `Done` /
    `StreamBlocked` also break with the queue non-empty.
  - documented chain (conn_actor.rs:444-447): non-empty queue ⇒ no pull next
    tick ⇒ outbound channel fills ⇒ producer (the relay's `send().await` into
    the channel) BLOCKS ⇒ the relay stops reading the backend ⇒ upstream read
    pauses. This is the end-to-end backpressure hyper's H2 path is missing:
    here the drain is window-gated (it honors quiche `StreamBlocked` / partial
    writes), so the producing channel genuinely fills.

INBOUND (client→gateway): the body-channel read-gate (conn_actor.rs:881-885:
`recv_body` only while `tx.capacity() > 0`) means the actor stops reading the
quiche stream when the channel is full → quiche's transport flow control
backpressures the client. The plan reuses this verbatim for the tunnel inbound
leg (s27-ws-h3-plan.md:49-51).

Therefore an H3 WS adapter built on two bounded mpsc channels draining through
this actor (plan Stage B) parks the relay's writes when the quiche stream can't
send, exactly the chain that is BROKEN on hyper-H2. WS-over-H3 would NOT inherit
F-S27-2, PROVIDED the implementation actually wires the outbound channel through
`drain_resp_channels` / `drain_streams_to_conn` (the empty-queue gate +
partial-tail retention) rather than a fire-and-forget drain. (Caveat: this is a
property of the PLANNED design as written; it must be re-verified on the actual
adapter with an R8 stalled-reader plateau test — plan Stage E line 69 already
calls for one.)

---

## Corrected scope + severity of F-S27-2

- TRANSPORTS: **WS-over-H2 ONLY** (RFC 8441 extended CONNECT). WS-over-H1
  (RFC 6455 Upgrade) is BOUNDED and NOT affected. Planned WS-over-H3 does NOT
  inherit it (Task 4).
- SHIPPED?: **YES on H2.** The production binary wires `with_websocket` on the
  H2 proxy (crates/lb/src/main.rs:1014, gated on a `[websocket]` config block)
  and advertises extended CONNECT unconditionally
  (crates/lb-l7/src/h2_proxy.rs:804 `enable_connect_protocol()`). So when
  WebSockets are enabled, a WS-over-H2 client reaching a chatty/large-volume
  backend (or a malicious backend pushing at a slow/stalled WS-over-H2 client)
  forces the gateway to buffer the producer's stream unbounded in RAM, per
  tunnel; N tunnels multiply it. Remote, low-cost memory-exhaustion DoS.
- SEVERITY: HIGH stands, but RESCOPED to the H2 transport. The verifier's
  "identical on shipped H1" inflation is incorrect; the H1 half of the original
  finding is a FALSE POSITIVE.
- ROOT CAUSE (corrected): NOT `WsConfig::tungstenite_config().max_write_buffer_size`.
  It is hyper's upgraded extended-CONNECT write path
  (`H2Upgraded` → mpsc(1) → `UpgradedSendStreamTask::tick` →
  `h2::SendStream::send_data` unconditional, no window gating) over h2's
  documented unbounded send buffer. Out of reach of any `WsConfig`-level change.
- KEEP (ws-eng's landed changes; correct & in-scope, NOT the H2 fix):
  the defensive `max_write_buffer_size` bound and the
  `timeout(read_frame, send().await)` → Close 1008 anti-hang guard. They are
  load-bearing on H1/raw transports (reclaim a wedged write) and harmless on H2.
- ACTION: R6-escalate the TRUE H2 backpressure fix to the owner (requires the
  option-B rearchitecture — raw `h2::SendStream` window-gated writes). Not
  tractable this session.

## Measurement provenance (R15)
Every number above is from a COMPLETED test run, stdout captured this session.
H1 17/2048 (x3, bound on/off/on). H2 1472/2048 (bound off) and 1504/2048
(bound on). Source citations are vendored file:line under
~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/. Probe edit to
ws_proxy.rs reverted; `git diff --quiet crates/lb-l7/src/ws_proxy.rs` = clean.
No suite source/test was committed.
