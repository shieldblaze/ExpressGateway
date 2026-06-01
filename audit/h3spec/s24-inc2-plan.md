# S24 — INC-2 (E1 ingress → quiche::h3) implementation plan

**Branch:** `feature/quiche-h3-migration-s23` (S24 continues it; base tip `d9417981`).
**Scope:** INC-2 = **E1 ingress ONLY** (egress stays raw `conn.stream_send`, proven
to coexist by INC-1 Experiment 4). Pre-authorized per S23 map + mission R7.
**Author/verifier:** lead implements; an **independent verifier subagent**
(fresh context) confirms every gate from completed logs (R5 author≠verifier).
Team-structure note: the mission lists lead/migration-eng/builder-1/verifier
agents; given harness instability this session, the load-bearing rule
(author≠verifier) is preserved via a dedicated independent verifier subagent
while the delicate hot-path surgery is done by the context-loaded lead. The
KEEP-surface must remain bit-identical; only framing changes (R3).

---

## 1. What INC-2 replaces vs. keeps (code-grounded, `conn_actor.rs`)

### REPLACE (ingress framing — the hand-rolled lb-h3 read side)
`poll_h3` (`:790-1237`) today does, per tick:
1. `flush_pending` for all body_pending streams (`:820-823`) — KEEP (channel plumbing).
2. `conn.readable()` loop (`:825`):
   - body-phase streams → `drain_body_stream` (`:835-838`)
   - uni streams (sid%4≠0) → drain+discard (`:852-856`)
   - request bidi: `conn.stream_recv` + `StreamRxBuf::feed` → HEADERS decode (`:857-1141`)
   - `Ok(None)`/FIN, `FeedError::{FrameUnexpected,QpackDecompressionFailed,Decode}`,
     `StreamReset|StreamStopped` arms (`:1143-1233`)

After INC-2 the read side is driven by a single `h3_conn.poll(conn)` event loop:
| quiche::h3 Event | Action |
|---|---|
| `Headers{list, ..}` | decode to `Vec<(String,String)>` (via `NameValue`), then the **KEEP** path: `validate_request_pseudo_headers` → authority → spawn cell task (h2/h3/h1/inline-400) + body channel register. `fin` on the event ⇒ bodyless. |
| `Data` | `recv_body` into the bounded request-body channel **only while `tx.capacity()>0`** (INC-1 Exp1 R8 gate). Not reading = peer paused. |
| `Finished` | `ReqBodyEvent::End { trailers: <from a trailing Headers event> }` |
| `Reset(code)` | **F-MD-4 point** → `ReqBodyEvent::Reset` (never clean EOF) |
| `GoAway`/`PriorityUpdate` | ignore (quiche handles control-stream rules natively) |

### KEEP (do NOT touch — the proxy value-add)
- `validate_request_pseudo_headers` (quiche does NOT validate pseudo-headers → #12–15 stay gateway-side).
- `H3Request::from_headers`, authority `lb_core::authority::validate`.
- The three cell-spawn blocks (h2_backend `:935`, h3_backend `:1010`, h1 `:1090`, inline-400 `:913`) — verbatim; they consume a decoded header list + `H3Request`, framing-agnostic.
- The bounded request-body channel (`btx/brx`, depth `H3_BODY_CHANNEL_DEPTH=8`), the response channels, `StreamTx`, **all egress** (`drain_streams_to_conn`, `drain_resp_channels`, `reap_client_cancelled_responses`) — INC-2 does not touch egress (Exp 4 coexist proof).
- `H3_*` error constants — still raised by the KEEP path (pseudo-header reject `H3_MESSAGE_ERROR`; egress abort `H3_INTERNAL_ERROR`). quiche raises #11/#16-22/#24 itself.

### DELETE (only the now-dead ingress framing; with their moot unit tests, R5)
- `StreamRxBuf` (`h3_bridge.rs:346-709`): `feed`, `feed_body`, `try_parse_frame_header`, `retained_bytes`, `is_too_large`, `RxPhase`, `BodyParse`, `BodyItem`, plus `FeedError`.
- In `conn_actor.rs`: the `conn.readable()`/`stream_recv`/`feed` loop, the uni-stream drain, `drain_body_stream`, `decode_into_pending`, `flush_pending`, `record_retained_for_stream` (re-pointed — see §3).
- **NOTE (R5):** deletion of `StreamRxBuf` etc. only happens once the quiche::h3 ingress replaces every caller. The body-phase parser `feed_body` and the HEADERS `feed` are the dead code. `lb_h3` crate itself is NOT deleted in INC-2 (egress `encode_h3_*` still uses `lb_h3::{QpackEncoder, encode_frame}`; E2 still uses it). lb-h3 deletion = INC-5/S25.

---

## 2. The actor wiring (the one structural change in `run_actor`)

`run_actor` (`:186-359`) holds an `Option<quiche::h3::Connection>` (call it `h3`):
- Build once, after `conn.is_established()`, via
  `quiche::h3::Connection::with_transport(&mut conn, &build_server_h3_config()?)`
  (INC-0 config; INC-1-proven). On error: log + close connection (fail safe).
- `with_transport` sends SETTINGS + opens the server control + QPACK enc/dec uni
  streams itself → the hand-rolled uni-stream drain is **deleted** (quiche owns them).
- `poll_h3(&mut conn, h3.as_mut().unwrap(), …)` replaces the old signature; the
  body-phase per-tick re-attempt (today line 835's `drain_body_stream` call for
  every `body_tx_by_stream` stream) becomes: for every stream still in body phase,
  attempt `recv_body` gated by channel capacity — **independent of whether poll
  surfaced a Data event this tick** (this is how today's code already works and it
  sidesteps quiche's edge-triggered Data re-arm: we always retry while the channel
  has freed capacity). This is INC-2's #1 correctness risk (R8/backpressure).

### Egress unchanged
`drain_streams_to_conn` still raw-`stream_send`s `RespEvent::Bytes` on the request
bidi stream. Exp 4 proved quiche::h3 ingress + raw bidi egress coexist on one conn.
`reap_client_cancelled_responses` uses `conn.stream_writable` — transport-level,
unaffected by h3 ownership. (INC-3/S25 migrates egress to `send_response`/`send_body`.)

---

## 3. R8 request gauge re-point (test-gauges) — MUST stay non-vacuous

Today `record_retained_for_stream` sums `StreamRxBuf::retained_bytes` + `body_pending`
bytes + channel occupancy. After INC-2 there is no `StreamRxBuf` on the read path
(quiche owns frame reassembly; `recv_body` is caller-sized into a fixed `[u8; N]`).
Re-point the request gauge to: `body_pending` bytes + channel occupancy +
the fixed `recv_body` scratch buffer high-water (≤ one chunk). This is structurally
**smaller** than before (quiche's internal stream buffer is flow-control-bounded,
not proxy-retained), so the bound only tightens — but the verifier must prove it
**non-vacuous** (gauge fires, channel genuinely saturates on a ≥1 MiB body, peak
body-size-independent: 1 MiB vs 4 MiB delta ≤ 1 chunk), exactly as INC-1 Exp1 did
on the experiment path, now on the **actual migrated `poll_h3`**.

---

## 4. F-MD-4 mapping (INC-2's highest-risk correctness property, R13)

- Today: `stream_recv` → `Err(StreamReset|StreamStopped)` mid-body → `ReqBodyEvent::Reset`.
- After: `h3_conn.poll` → `Event::Reset(code)` for the request stream → `ReqBodyEvent::Reset`
  (+ tear down per-stream state). A client RST mid-body MUST relay as a backend reset,
  NEVER a clean EOF (smuggling/desync guard). quiche may ALSO surface a mid-body
  cancel as `recv_body` → `Err` — handle both: any non-`Done` `recv_body` error or
  an `Event::Reset` ⇒ `ReqBodyEvent::Reset`.
- Verify: (a) ×3 gate green, (b) isolation burst ≥50 iter (`h3h3`/`h3h2` RST burst
  current_thread tests), (c) load-bearing negative control (a clean-EOF case that
  must NOT reset).

---

## 5. Increment gating (INC-2 commit bar)

1. Build: `cargo check/clippy --all-targets --all-features -D warnings` + fmt clean.
2. **Safety net:** the 69 H3 wire tests (h3_h1*, h3_h2*, h3_h3*, round8 authority,
   graceful_close, s16/s19 Mode B) pass against migrated ingress.
3. **R12:** all H3-front cells (H3→H1, H3→H2, H3→H3) + Mode B re-verified (single ingress).
4. **R8:** request gauge non-vacuous + body-size-independent on migrated `poll_h3`.
5. **R13:** F-MD-4 RST burst (×3 + ≥50-iter isolation + negative control).
6. Full workspace ×3 deterministic (R1).
7. Commit + push to origin (R5). Update `s24-report.md`.

If quiche::h3 ingress cannot express a KEEP need (e.g. pseudo-header pass-through,
trailer surfacing, or the inline-400 authority path) → escalate (R7) BEFORE deleting
the corresponding hand-rolled code.

---

## 6. Two KEEP-surface behaviors that MOVE into the new ingress loop (found in code review)

### (a) The 64 MiB total-body cap (413 / F-CAP-1) — was inside `feed_body`
`StreamRxBuf::feed_body(chunk, MAX_REQUEST_BODY_BYTES)` (`h3_bridge.rs:579`) latched
`too_large` on cumulative `body_seen > 64 MiB` → `BodyItem::TooLarge` →
`decode_into_pending` sent `ReqBodyEvent::Reset` → the cell consumer maps `Reset`→
**413** (`h3_bridge.rs:2244-2248`, `:2697`). Deleting `feed_body` deletes this cap, so
the **new ingress loop MUST track `body_seen` per stream** and, on
`body_seen > MAX_REQUEST_BODY_BYTES`, send `ReqBodyEvent::Reset` + tear down (do NOT
forward further body). Gate: `t4_oversized_body_yields_413_and_upstream_not_completed`
(`h3_h1_stream_body_e2e`) + the F-CAP-1 over-cap-arm-taken check (memory
`fcap1-overcap-arm-backpressure-masked`: verify the cap branch is TAKEN, not masked
by backpressure).

### (b) Request trailers — now a SECOND `Event::Headers`, not a `feed_body` item
quiche surfaces a post-DATA trailing HEADERS frame as another `Event::Headers{list}`
(it allows `headers_received_count == 2`, `mod.rs:2932`). The old path captured these
in `feed_body` as `BodyItem::Trailers` and (i) **rejected any pseudo-header in the
trailer section** (`feed_body:635` → `Err` → `ReqBodyEvent::Reset`). New loop: a
2nd `Headers` event on a stream already in body phase ⇒ treat `list` as trailers:
reject if any name starts with `:` (→ `ReqBodyEvent::Reset`), else stash and attach
to the `ReqBodyEvent::End { trailers }` emitted on the subsequent `Finished`.

## 7. New per-stream ingress state (replaces `StreamRxBuf` on the read path)
A small `HashMap<u64, H3IngressState>` (or fold into existing maps):
`{ body_seen: usize, pending_trailers: Vec<(String,String)> }`. `body_tx_by_stream`
still marks "in body phase". The `recv_body` scratch is a fixed `[u8; H3_BODY_CHUNK_MAX]`.

## 8. The poll/recv_body control flow (edge-trigger-safe, R8)
quiche `poll` is **edge-triggered, one-event-per-call**; `Data` fires once
(`mod.rs:2797` `try_trigger_data_event`) and re-arms only after the stream drains to
`Done`. Since the R8 gate STOPS `recv_body` while the channel is full (not draining to
`Done`), the loop MUST, **every tick**, re-attempt a capacity-gated `recv_body` drain
for every body-phase stream — independent of whether `poll` surfaced a fresh `Data`
this tick (exactly what today's `drain_body_stream` does). Structure of new `poll_h3`:
1. **Re-arm pass:** for each `body_tx_by_stream` sid, `recv_body` into the scratch
   while `tx.capacity()>0`; `try_send` each read; track `body_seen` (cap→Reset);
   `recv_body` returning the fin / `conn.stream_finished` ⇒ rely on the `Finished`
   event for End (don't double-emit).
2. **Event pass:** `loop { match h3.poll(conn) }` — `Headers` (initial vs trailer),
   `Data` (drain that sid, capacity-gated — redundant-but-safe with pass 1),
   `Finished` (emit `End{stashed trailers}`, tear down), `Reset(code)` (F-MD-4 →
   `ReqBodyEvent::Reset`, tear down), others ignored, `Err(Done)` break,
   `Err(other)` ⇒ quiche has already `conn.close`d (it owns #11/#16-22/#24); log+break.
