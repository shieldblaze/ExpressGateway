# S18 Phase 1 — diag-eng brief (three-bucket diagnosis of CF-S16-RELAY-STALL)

You are **diag-eng**. FRESH judgment. Your job: pin the stall root to ONE of three
buckets by **measured state**, then run ONE decisive intervention. Do NOT ship a fix.
Author≠verifier: the verifier will independently reproduce your bucket afterward.

## What is already PROVEN (do NOT re-derive, do NOT retry)
On a stall (reproduced 6/6 in S16+S17): the relay reads the WHOLE backend response and
reclaims its stream table (`u2c.done`, `up_stream_finished=true` all sids); exactly ONE
upstream→client stream is short at the client by a VARYING amount (`short_by` 396–44458 B,
sid varies = lost-flight signature); the relay's client-facing `params.conn` NEVER
retransmits (`CL-PATH total_pto=0 retrans=0 stream_retrans_bytes=0` on EVERY stall) while
`cl_sent>cl_acked`; the LB client conn idle-closes at ~20s (`break_reason=client_closed`);
the test hits its 25s `RELAY_BUDGET`.

FIVE refuted hypotheses — do NOT retry: (1) tick/wake cadence, (2) test-driver
single-datagram drain, (3) wake-latency `.min(IDLE_TICK)`, (4) backend-PTO-never-fires
(backend PTO DID fire+retransmit), (5) relay arm-E `upstream.conn.on_timeout()` starvation
(REFUTED: forcing BOTH relay on_timeouts every iter left stalls at baseline, client leg
still total_pto=0).

## KEY NEW LEAD OBSERVATIONS (grounding your measurement — verify, don't assume)
1. quiche 0.28 `congestion/recovery.rs::set_loss_detection_timer`: it **clears** the loss
   timer when `bytes_in_flight.is_zero() && peer_verified_address`. So "no PTO armed"
   (total_pto=0, on_timeout forced thousands of times yet never fires) ⟺ quiche believes
   **bytes_in_flight == 0** for `params.conn`. That means the short tail is NOT in flight
   — it is either BUFFERED-NOT-SENT or removed-and-not-rearmed. This makes the **stream
   send-buffer length + `stream_capacity(sid)`** the decisive discriminators.
2. Relay loop (`raw_proxy.rs::run_dual_pump`): `drain_conn_send` runs every iter
   (loops `conn.send()` to Done, ignores pacing `info.at`). The `RELAY_TICK` (2ms) wait
   clamp is gated on `!streams.is_empty()` — once all streams are RECLAIMED, the loop
   parks on `params.conn.timeout()` (= ~20s idle timer when no loss timer armed). So a
   buffered tail with no near-term timer waits until idle-close.
3. **The test CLIENT driver never calls `client_conn.on_timeout()`** (multistream
   :521-609; `flush`+`try_recv_one` only). It drives nothing but a 5ms recv + ACK flush.
   So any client→relay packet that is LOST — an ACK, or (critically) a **MAX_STREAM_DATA
   / MAX_DATA flow-control update** the client emits after reading — is **NEVER
   retransmitted by the client** (its PTO never gets serviced). S17's probe only forced
   the RELAY's on_timeouts; it NEVER touched the client. This is an OPEN, untested path.

   Candidate mechanism (FALSIFY by measurement, do not assume): the relay fills the
   client's stream/conn receive window → quiche stalls `send()` with the tail BUFFERED
   (bucket a, flow-control flavor, `stream_capacity==0`, bytes_in_flight→0, loss timer
   cleared) → the client reads, quiche queues a MAX_STREAM_DATA, the client flushes it →
   that update is LOST → the client never retransmits it (no on_timeout) → the relay never
   learns the window opened → mutual silence → 20s idle-close. A REAL client runs its own
   loss recovery and WOULD retransmit the update, so this would be a TEST-HARNESS artifact,
   not a production relay defect. PROVE or REFUTE it.

## TASK A — instrument + capture the bucket (env-gated, worktree-only, uncommitted)
Use your isolated worktree + a dedicated `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-s18-diag-target`.
`quiche` is crates.io `0.28.0`. To read internals, add a worktree-only
`[patch.crates-io] quiche = { path = "<copy of 0.28.0 src>" }` to the workspace root
`Cargo.toml` and add small `pub` debug accessors on `quiche::Connection` (copy the source
from `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/quiche-0.28.0`). Expose, for a
given sid on `params.conn`:
- `bytes_in_flight` (recovery), `cwnd`, the `loss_detection_timer` Option<Instant> (None vs
  future), `stream_capacity(sid)` (already public), and the **stream send-side buffered
  length** (bytes accepted by `stream_send` but not yet emitted — `streams.get(sid).send.len`
  or equivalent; find the right field).
- On the CLIENT conn at the stall: whether a MAX_STREAM_DATA/MAX_DATA frame is pending or
  was sent+lost (if reachable); at minimum its `bytes_in_flight` + loss timer.
Gate ALL of it behind an env var (e.g. `EG_RELAY_DIAG3`), OFF by default. Add an at-stall /
at-EXIT dump in `run_dual_pump` for the short sid (the relay loop exits at ~20s idle; the
at-EXIT dump captured S17's decisive state — keep that approach).

Run a **≥75-iter isolation burst** of `s16_b2_multistream` AND a burst of the
`s16_b2_backpressure` resume path (both affected). FOREGROUND or background-to-completion;
read the FULL completed log before citing any number (R15). Capture, at ≥3 distinct stalls
across BOTH paths, for the short sid on `params.conn`:
`send_buf_len`, `stream_capacity`, `bytes_in_flight`, `cwnd`, `loss_timer`.

Classify into EXACTLY one bucket:
- (a) BUFFERED-NOT-SENT: `send_buf_len > 0` (≈ short_by). Sub-split: flow-control
  (`stream_capacity==0`) vs cwnd (`bytes_in_flight >= cwnd`) vs relay-not-flushing
  (`stream_capacity>0 && bytes_in_flight<cwnd`).
- (b) IN-FLIGHT-NO-PTO: `send_buf_len==0`, `bytes_in_flight>0`, `loss_timer==None`.
- (c) RETRANSMIT-SUPPRESSED: loss timer armed + fired, retrans still 0.

Read quiche `congestion/recovery.rs` (`set_loss_detection_timer`, `pto_time_and_space`,
`detect_lost_packets`) against the captured state to explain WHY.

## TASK B — ONE decisive intervention (Koch-style), measured
Based on the bucket, run the single most discriminating intervention and measure the stall
rate (≥75 iters, both paths, full log):
- If (a) flow-control flavor: add `client_conn.on_timeout()` (driven by `conn.timeout()`)
  to the TEST client driver loop (a throwaway test-side change in your worktree) and remeasure.
  If the stall → **ZERO**, the root is "test client never runs loss recovery"
  (test-harness artifact; production relay correct). If it persists, root is relay-side.
- If (a) relay-not-flushing: force the relay to keep the RELAY_TICK clamp while
  `params.conn` has un-flushed/un-acked client-leg data and remeasure.
- If (b)/(c): the appropriate quiche-state intervention.
State pre- and post-intervention stall rates with cited completed logs.

## HARD CAP / OUTPUT
One focused pass. Output `audit/quic/s18-logs/p1-diag-findings.md`: the bucket (a/b/c +
sub-split) with the captured numbers AT ≥3 stalls across BOTH paths, the recovery.rs
predicate that explains it, the decisive intervention's pre/post stall rate (cited logs),
and a one-line root statement + recommended fix LOCUS (test-side vs relay-side vs quiche).
Keep the shared checkout PRISTINE (verify `git status` clean, no `EG_RELAY_DIAG3` strings
leak out, no `git stash`). Report log paths. Do NOT write a production fix.
