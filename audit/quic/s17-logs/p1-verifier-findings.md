# S17 Phase 1 — CF-S16-RELAY-STALL VERIFIER findings (independent second leg)

Author: verifier (independent; trusts no other agent's prose). One hard-capped
pass. Every number below cites a COMPLETED, fully-read burst log (R15). Work done
in an isolated worktree on throwaway branch `s17-verifier-instr` off
`feature/quic-proxy-s17` @ `bbfd79a1`; shared checkout `/home/ubuntu/Code/ExpressGateway`
left PRISTINE (verified: 0 `EG_RELAY_DIAG2` strings, `git status` clean, still on
`feature/quic-proxy-s17`). No `git stash`. Dedicated target dir
`/home/ubuntu/Code/eg-s17-target` (binary `s16_b2_multistream-d9bd20c52927799e`).

Instrumentation (env-gated `EG_RELAY_DIAG2`, OFF by default; `EG_RELAY_PROBE` for
Task 3). Three sites: (1) relay `run_dual_pump` — per-arm counters, a backend-acked-
frozen during-stall trigger, an at-EXIT dump (`RELAY-EXIT-DUMP`/`RELAY-STALL`) with
per-sid u2c/c2u half-state (reclaim-latched) + both legs' conn/path stats +
`stream_finished`/`stream_capacity`, and a `break_reason`; (2) echo backend —
`BACKEND-STALL` fingerprint; (3) test CLIENT driver — `CLIENT-STALL` per-stream
short/FIN dump near the 22.5 s mark. The relay during-stall trigger never fired
(`stall_dumped=false` everywhere) because the relay pump loop EXITS at ~20 s (client
idle-timeout) before the 2.5 s frozen detector — the at-EXIT dump is what captured
the decisive state.

Evidence logs (all completed + fully read):
- `/home/ubuntu/Code/eg-s17-evidence/verifier-baseline-50.log` (50 iters, 1 stall)
- `/home/ubuntu/Code/eg-s17-evidence/verifier-baseline-75.log` (75 iters, 3 stalls)
- `/home/ubuntu/Code/eg-s17-evidence/verifier-baseline-clientdump-60.log` (60 iters, 6 stalls; full client+relay+backend)
- `/home/ubuntu/Code/eg-s17-evidence/verifier-probe-75.log` (75 iters, 4 stalls; PROBE active)

---

## TASK 1 — independent reproduce + per-fact agree/disagree

Reproduced the stall from my own instrumented binary. Baseline stall rate
(no probe), combined: **11 stalls / 185 iters ≈ 5.9%** (1/50, 3/75, 6/60). Per-burst:
2% / 4% / 10%. Variable run-to-run, lower than diag-eng's 12% headline but the SAME
phenomenon (25 s `RELAY_BUDGET` panic at `s16_b2_multistream.rs:695`). AGREE it
reproduces; the rate is noisy and burst-dependent.

Per diag-eng fact (cite = my logs):

| diag-eng fact | my verdict | my cite |
|---|---|---|
| Backend `peer_fin_seen=true` ALL sids at stall | **AGREE** | clientdump-60.log all 6 `BACKEND-SID` blocks, e.g. iter2 `BACKEND-SID=0..16 peer_fin_seen=true our_fin_sent=true stream_finished=true` |
| Backend `queued_left=0`, `our_fin_sent`/`stream_finished=true` | **AGREE** | same lines |
| Backend has ~100-164 KB un-acked response tail; backend PTO FIRED + retransmitted (PTO NOT flat) | **AGREE** | baseline-75.log iter32 `BACKEND-STALL ... sent_bytes=1009214 acked_bytes=849340 ... total_pto=2 retrans=139 stream_retrans_bytes=122685`; every stall `total_pto>=2` |
| Relay keeps RECEIVING backend pkts through the stall but backend `acked` stays FROZEN | **AGREE** | clientdump-60.log iter2 `UP-PATH recv=1787` (climbs) yet `up_acked_bytes=841666 < up_sent_bytes=896609`; relay `arm_c=1785` firing |
| Relay `arm_e` (upstream `on_timeout`) ≈ 0 / never serviced | **AGREE** | every `RELAY-FINAL` across all baseline bursts shows `arm_e=0` (and `arm_d` 0 or 1); 0/185 baseline iters had `arm_e>0` |
| GAP: relay per-stream state DURING stall not captured | **CLOSED** (Task 2) | see below |

DISAGREE with NOTHING in diag-eng's captured facts. I DISAGREE only with the
implied remedy (that servicing arm-E fixes it) — refuted in Task 3.

---

## TASK 2 — the decisive bit: u2c STUCK (i) or ACK/timer loop (ii)?

**Answer: NEITHER as diag-eng framed it. The relay u2c is DONE (delivered all
backend bytes into the relay AND reclaimed), and the actual short fall is on the
CLIENT LEG: the relay's `params.conn` sends the response tail to the client, a tail
flight is LOST, and the relay NEVER retransmits it.** This is closest to (ii) but
the stalled loop is the relay's CLIENT-leg loss recovery, not its backend-ACK loop.

Captured at every stall (relay at-EXIT dump + client dump,
`verifier-baseline-clientdump-60.log`):

- The relay's u2c half for every sid is reclaimed `done` (reclaim ⟺ both halves
  done; `up_stream_finished=true` for all sids ⇒ the relay READ the full backend
  response+FIN). So u2c is NOT stuck reading from the backend — NOT case (i).
  Cite: iter2 `RELAY-STALL SID=8 u2c[done=true ...] up_stream_finished=true`.
- **Exactly ONE stream is short at the client, every stall** (the short sid + amount
  VARY → a lost-flight signature, not a deterministic logic bug):
  - iter2  `CLIENT-STALL SID=8  got_len=172362 want=200000 short_by=27638 got_fin=false client_stream_finished=false client_readable=false`
  - iter12 `CLIENT-STALL SID=4  short_by=396`
  - iter33 `CLIENT-STALL SID=8  short_by=13901`
  - iter48 `CLIENT-STALL SID=16 short_by=4271`
  - iter50 `CLIENT-STALL SID=4  short_by=44458`
  - iter52 `CLIENT-STALL SID=16 short_by=15212`
  The client is `client_readable=false` on the short stream → it has drained
  everything the LB delivered and is simply waiting for bytes that never arrive.
- The relay's CLIENT leg shows the lost tail **never retransmitted**:
  `RELAY-STALL CL-PATH ... lost=53 retrans=0 total_pto=0 stream_retrans_bytes=0`
  (iter2). Across ALL stall CL-PATH lines, `retrans=0` and `total_pto=0` while
  `lost` is 13/43/53/61 — quiche DETECTED the loss but the relay's `params.conn`
  did NOT retransmit. `cl_sent_bytes > cl_acked_bytes` (iter2 868495 > 828155).
- The relay loop EXITS via `break_reason=client_closed` at `pump_elapsed_ms≈20453`
  — the LB's client conn IDLE-TIMED-OUT at the 20 s `max_idle_timeout` because it
  stopped making progress, dropping the stream forever; the test then hits its 25 s
  budget. (`cl_is_closed=true` in the RELAY-STALL line.)

So: relay reads the whole backend response, hands it to `params.conn`, `params.conn`
sends it, the last flight to the client is lost, and **the relay's client-leg
PTO/loss-retransmit never runs**, so the client is permanently short → idle close →
budget wall. The `up_acked` freeze diag-eng saw is a downstream SYMPTOM (the relay
has no NEW ack-eliciting data for the backend once the stream is done, so the
backend's `acked` can't advance and the backend retransmits into a relay that is
itself wedged), not the trigger.

---

## TASK 3 — decisive intervention (Koch-style): service on_timeout unconditionally

PROBE (`EG_RELAY_PROBE=1`): after every `select!` wake, unconditionally call
`upstream.conn.on_timeout()` AND `params.conn.on_timeout()` (no-op when no timer
due), removing the arm-E (and arm-D) biased-select starvation.

- **Pre-probe baseline stalls: 11 / 185 iters (≈5.9%)** — cite: the three baseline
  logs (1/50, 3/75, 6/60).
- **Post-probe stalls: 4 / 75 iters (≈5.3%)** — cite: `verifier-probe-75.log` stall
  iters 10, 27, 44, 51 (`RC=101`); all 4 carry the SAME `BACKEND-STALL` fingerprint
  and `CL-PATH total_pto=0 retrans=0`.

**The probe was genuinely active** (not a no-op miswire), and on every probe stall
the loop ITERATED actively through the whole 20 s window (NOT dormant):
- every probe `RELAY-FINAL` has `probe=true` (75/75);
- the loop ran `iters=3650..4590` with `arm_c≈1763..2477` over `pump_elapsed_ms≈20.5k`
  — so `params.conn.on_timeout()` was forced thousands of times during the stall;
- the upstream PTO it forced is VISIBLE on ALL 4 probe stalls:
  `UP-PATH total_pto=2..5 retrans=3..9 stream_retrans_bytes=324..3918`
  (vs baseline `UP-PATH total_pto=0`).

Yet **stalls PERSIST at ≈baseline** and on ALL 4 probe stalls the CLIENT leg STILL
shows `CL-PATH total_pto=0 retrans=0 stream_retrans_bytes=0` while `cl_sent_bytes >
cl_acked_bytes` (e.g. iter10 884913 > 844332, ~40 KB unacked). So even with the loop
spinning fast AND `params.conn.on_timeout()` forced every iteration, **the
client-leg PTO is never ARMED** — forcing `on_timeout` cannot retransmit the wedged
client-leg tail. (Note: probe-stall `CL-PATH lost=0` — the ~40 KB is unacked
in-flight/buffered with NO loss-recovery timer scheduled, not even declared lost.)
This is the structural reason the proposed fix cannot work.

**Result: NEGATIVE. Removing arm-E (and arm-D) starvation does NOT clear the stall.**
Arm-E starvation is REAL (confirmed Task 1) but it is NOT the mechanism of this
stall — DISPROVEN BY INTERVENTION. diag-eng's own §6 already suspected arm-E was
"NOT directly" causal; my experiment confirms it conclusively and rules it out as
the fix.

---

## VERDICT

**MECHANISM (pinned to a class, sharper than diag-eng): a CLIENT-LEG lost-tail
loss-recovery failure.** On a stall, exactly one upstream→client stream loses a
tail flight on the LB→client path; the relay's `params.conn` detects the loss
(`CL-PATH lost>0`) but **never retransmits it** (`total_pto=0`, `retrans=0`,
`stream_retrans_bytes=0` on EVERY stall), so the client stays permanently short,
the LB client conn idle-closes at 20 s (`break_reason=client_closed`), and the test
hits its 25 s budget. The relay had already read the full backend response
(`up_stream_finished=true` all sids) and reclaimed its stream table — so it is NOT a
relay request-FIN loss, NOT a u2c read-stuck, NOT a backend retransmit-cadence bug.

**The proposed fix "service upstream `on_timeout` unconditionally" is REFUTED** — it
does not clear the stalls (Task 3, 4/75 with the probe active). So the production
fix is NOT "call arm-E more often."

**Why the client-leg PTO never fires is the remaining unknown** and it is a genuine
quiche loss-recovery / `params.conn` send-state question (loss DETECTED via ACK gap
but no retransmit and no PTO armed for the lost stream tail). I did not isolate it
to a quiche line in one pass, and the intervention that the brief specified does not
move it. The during-stall RELAY/CLIENT state is UNAMBIGUOUS (captured, reproducible,
6/6), but the suspected cause's removal did NOT remove the effect.

**Per the HARD CAP → ESCALATE R6 (genuinely-large).** The CLASS is pinned and the
brief's candidate fix is refuted by controlled experiment, but the exact failing
mechanism (LB `params.conn` not retransmitting a lost, already-`stream_send`-accepted
client-leg tail on a relay-FIN'd stream) is a deeper QUIC loss-recovery question
that needs its own focused pass before a builder writes a fix. Do NOT land
"service on_timeout" as the fix — it is empirically insufficient.

### Direction for the R6 / builder pass (NOT a fix I am landing)
- The fix must make the LB's `params.conn` actually RETRANSMIT the lost client-leg
  tail (and/or keep the relay actively pumping/loss-recovering the client leg after
  it has reclaimed the stream). Forcing `on_timeout` is necessary-but-insufficient
  because no PTO is armed; investigate WHY quiche detects the client-leg loss
  (`lost>0`) yet schedules no stream retransmit on a relay-FIN'd stream — candidate
  areas: cwnd collapse (CL-PATH `cwnd` as low as 13500 ≈ 10 pkts) blocking the
  retransmit while no PTO probe is sent; or the relay ceasing to `drain_conn_send`
  the client leg effectively once `streams.is_empty()` drops the RELAY_TICK clamp.
- **Echo backend fix is NOT what clears this stall.** The intervention space here is
  the LB relay's CLIENT leg, not the backend. (The backend's retransmit-into-frozen-
  acked is a symptom.) A backend `on_timeout` change alone would not deliver the
  missing client-leg tail. So the production fix is on `run_dual_pump`'s client-leg
  send/loss-recovery, confirming diag-eng's "production relay implicated, NOT
  test-only" conclusion — but via a DIFFERENT mechanism than arm-E.

### Hygiene
Shared checkout PRISTINE (verified). Instrumentation lives only in worktree branch
`s17-verifier-instr` (uncommitted, throwaway). No `git stash`. Target dir
`/home/ubuntu/Code/eg-s17-target` may be removed.
