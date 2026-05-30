# S17 Phase 1 — CF-S16-RELAY-STALL diagnosis brief (lead → diag-eng)

> **Goal: a PROVEN mechanism from captured, read-to-completion instrumentation
> — not a guess.** HARD CAP: one focused diagnostic pass. If the mechanism is
> not proven (captured + independently reproduced by the verifier + confirmed)
> in this pass, STOP and escalate as R6-genuinely-large. Do NOT spin a fourth
> hypothesis loop. Do NOT attempt a fix in this phase.

## The defect (S16 evidence, do not re-derive)
`tests/s16_b2_multistream.rs` — 5 concurrent client bidi streams (ids 0/4/8/12/16,
sizes 9/60/200/400/130 KB), each echoed by a real quiche backend through the Mode B
relay (`run_raw_proxy_actor_for_test` → `run_dual_pump`). **~22 % stall on a QUIET
8-core box (11/50)**, hits the 25 s `RELAY_BUDGET` wall. Liveness only — byte-identical
when it completes. NOT load-triggered, NOT CF-SATURATION-1.

## DISPROVEN — do NOT retry (S16)
1. tick/wake-cadence (6 loop-timing variants all stalled at baseline).
2. receive-starvation / test-driver single-datagram-drain / "test-rig bug" (greedy
   drain still stalled 13/19; the "production relay CORRECT" claim was RETRACTED).
3. wake-latency cap `.min(IDLE_TICK)` (14/40 vs 9/40 — worse; select waits at the
   stall were ~26 ms, not the assumed ~20 s).

## Best current evidence — the SHARPENED mechanism (S16 UPDATE 3), your start point
Instrumented `run_dual_pump` half-state at a real stall (stream 8 in that capture; also
seen on stream 0 — varies run to run):
```
streams_len=1 cl[sent=2127 recv=2090 lost=39] cl_readable=0 cl_cap0=InvalidStreamState cl_fin0=true
sid=8 c2u[pend=0 fin_seen=true fin_sent=true done=true]    <- REQUEST leg COMPLETE
      u2c[pend=0 fin_seen=false fin_sent=false done=false] <- RESPONSE leg STUCK
```
Reading: 4 streams done; ONE stream's **response leg `u2c` is stuck** — the relay has
read NOTHING new from the upstream (`pend=0`) and never saw the backend's FIN
(`fin_seen=false`); nothing readable on either leg; `lost>0`; the connection is still
ACKing (recv climbs) but no stream progress; the loop is awake (~26 ms cadence), not
parked. **S16's own note: this "involves the hand-rolled echo BACKEND loop too (its
retransmit cadence)", not only `run_dual_pump`.** The capture is INCOMPLETE: it shows
client-leg stats only — NOT the upstream-leg path_stats nor the backend's send state.

## Lead's candidate hypothesis (prove OR disprove from captured data — your call)
**Tail-loss non-retransmission of the final response packet.** The stuck stream's last
response packet (carrying its FIN) is lost. Its recovery CANNOT use ACK-based
(packet-/time-threshold) loss detection because there is no LATER packet to be ACKed —
only the **PTO (time-threshold) timer** can recover a tail loss, and PTO fires only via
`on_timeout()`. Candidate culprit: the **test echo backend** (`spawn_echo_backend`,
`tests/s16_b2_multistream.rs:221-334`) calls `c.on_timeout()` **only in the `recv_from`
*timeout* branch** (lines 308/324-328). If the relay keeps sending packets (ACKs) to the
backend faster than `conn.timeout()`, `recv_from` keeps returning `Ok` BEFORE the
timeout → `on_timeout()` is NEVER called → the backend's PTO never fires → the lost FIN
packet is never retransmitted → the relay's `u2c` waits forever. This exactly matches
"both ends busy with ACK traffic" + `lost>0` + `fin_seen=false` + awake-but-no-progress.

This hypothesis is FALSIFIABLE — it predicts, at a stall:
- Backend has outstanding un-ACKed tail data (`stats.sent_bytes > stats.acked_bytes`),
  stream `sid` NOT finished/drained on the backend, and **`path_stats.total_pto_count`
  FLAT** (never increments) while its recv count climbs.
- Relay upstream-leg `path_stats`: NOT receiving new data for `sid`; `lost`/`retrans`
  consistent with the missing tail being on the backend→relay path.

If the captured data CONTRADICTS this (e.g. backend already retransmitted, PTO fired,
relay received the FIN but failed to forward it; or the relay is the one holding
un-retransmitted data), then the hypothesis is wrong — capture the TRUE state and report
it. Disproving this is as valuable as confirming it.

## Instrumentation to add (both sides), then capture ONE clean stall
Add behind a cheap env-gated flag (e.g. `EG_RELAY_DIAG=1`) so it is OFF in the normal
gate. **All instrumentation is REVERTED before Phase 1 closes — the tree must be
pristine for the fix phase.** Use `tracing`/`eprintln!` to a file; run FOREGROUND; read
the COMPLETE log (R15).

### A. Relay (`run_dual_pump`, `src/raw_proxy.rs`)
Add a stall detector: when `!streams.is_empty()` and no half made progress for ~K loop
iterations / ~T ms, dump ONCE per stall:
- For the stuck `sid`: `c2u`/`u2c` half-state (pend/fin_seen/fin_sent/done) — confirm
  which leg.
- **Upstream leg** `upstream.conn.path_stats()` (the missing piece): `recv sent lost
  retrans total_pto_count cwnd rtt sent_bytes recv_bytes`, plus `upstream.conn.stats()`
  (`acked_bytes`), `upstream.conn.stream_finished(sid)`, `upstream.conn.readable()`
  contains `sid`?
- **Client leg** `params.conn.path_stats()` + `stats()` (same fields).
- Per-`select!`-arm FIRE COUNTERS since last dump — especially **arm 5 (upstream
  `on_timeout`)** vs arm 3 (upstream `recv_from`) vs arm 4 (client `on_timeout`). (I
  suspect arm 5 is biased-starved when both waits == RELAY_TICK — verify whether the
  RELAY itself ever services its upstream timer. This is the production-immunity
  question, see below.)
- Current `client_wait` / `upstream_wait` and `upstream.conn.timeout()`.

### B. Echo backend (`spawn_echo_backend`, `tests/s16_b2_multistream.rs`)
Instrument the loop to dump (periodically once a stall is suspected, or on a deadline
approach):
- `c.path_stats()`: `sent lost retrans total_pto_count stream_retrans_bytes cwnd` and
  `c.stats()`: `sent_bytes acked_bytes lost_bytes`.
- Per stream `sid`: queued-bytes-left (`e.0.len()`), peer-FIN-seen (`e.1`), our-FIN-sent
  (`e.2`), `c.stream_finished(sid)`.
- **Counters**: number of `c.on_timeout()` calls vs number of `recv_from`→`Ok` since
  start, and the current `conn.timeout()` value. (The starvation ratio is the smoking
  gun: if on_timeout-count is ~0 while recv-Ok-count is large AND tail data is unacked →
  hypothesis CONFIRMED.)

### Capture protocol (R15)
- Build instrumented binary at full `-j` (`CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`,
  `CARGO_INCREMENTAL=0`).
- Run an isolation burst FOREGROUND (≥50 iters; the 22 % rate surfaces a stall quickly).
  Log to a file; read it to COMPLETION; the dump for at least one real stall must be in
  the completed log.
- A claim ("mechanism = X") must cite the exact completed log + the captured numbers.

## The PRODUCTION-IMMUNITY question (decides fix scope — REQUIRED output)
Even if the root cause is the test backend's retransmit cadence, you MUST determine
whether **`run_dual_pump` has the analogous defect against a REAL peer**:
- A real backend services its loss timers independently of recv, so it WOULD retransmit
  a tail loss — meaning the immediate fix would be **test-only** (make the echo backend
  service `on_timeout` regardless of recv, e.g. a `select!` timer arm like the relay).
- BUT: does the relay reliably service BOTH `params.conn.on_timeout()` AND
  `upstream.conn.on_timeout()`? With `biased` select and both waits capped at
  `RELAY_TICK` (2 ms), arm 4 (client timeout) may systematically win over arm 5
  (upstream timeout), starving the relay's UPSTREAM `on_timeout`. If a real backend
  relied on the RELAY to retransmit the REQUEST leg's tail (c2u), a starved arm 5 would
  be a genuine PRODUCTION liveness bug. Capture arm-5 fire-count at a stall to settle
  this. If real → it is a production fix (R12 / single-source), not test-only.

## Decision tree (report ONE of these, with cited numbers)
1. **Backend retransmit-cadence (test-only)** — backend holds unacked tail, PTO flat,
   relay services its own timers fine → fix is the echo backend; prove relay immune.
2. **Relay timer-servicing (production)** — relay fails to service `upstream.conn`
   on_timeout (arm 5 starved) or otherwise stalls forwarding → production fix, R12.
3. **Something else** — capture contradicts both; report the true captured state.
4. **Unproven in one pass** → escalate R6-genuinely-large; Mode B stays unpromoted.

## Author ≠ verifier
diag-eng CAPTURES; the verifier INDEPENDENTLY reproduces the stall and confirms the
mechanism from its OWN completed log before it is believed. No single-agent claim on
this bug (S16 fabrication precedent — R15).

---

## LEAD SHARPENING (S17, pre-dispatch — read AFTER the above)

Re-reading **both** S16 captures together changes the picture: there look to be **TWO
distinct loss-timer-starvation surfaces**, sharing ONE root pattern, and the stuck leg
**varies run-to-run** (stream 0 / 8 / 12 across captures). Do NOT conclude from a single
stall.

1. **UPDATE-3 capture** = RESPONSE leg `u2c` stuck, `fin_seen=false` → the backend's
   response tail (its FIN packet) is the missing flight → the **echo BACKEND** must
   PTO-retransmit it. Backend candidate = `on_timeout()` only in the recv-timeout branch
   (`s16_b2_multistream.rs:324-328`); a busy ACK stream keeps `recv_from` returning `Ok`
   before `conn.timeout()` elapses → backend PTO never fires. (Test-only.)
2. **Earlier hypothesis-2 fingerprint** (s16-report §CF, "prior fix agent") = the relay's
   **UPSTREAM leg ~63-90 KiB un-acked in flight, ONLY idle timer armed, no PTO/loss
   timer** → the relay's REQUEST tail to the backend is the missing flight → the **RELAY**
   must PTO-retransmit it on `upstream.conn`. Candidate = `run_dual_pump`'s `biased`
   `select!`: while `!streams.is_empty()`, BOTH `client_wait` and `upstream_wait` are
   clamped to `RELAY_TICK` (2 ms) and are EQUAL, so the client-`on_timeout` arm (line 449)
   wins every tie and the **upstream-`on_timeout` arm (line 452) is starved** → the relay
   never services `upstream.conn`'s loss/PTO timer while any stream is mid-transfer.
   (PRODUCTION.)

**Common root:** *whoever owns retransmission of a lost tail packet never services its
loss/PTO timer while both ends are busy with ACK traffic.* It surfaces on the relay
(arm-E starvation, production) AND on the test backend (recv-only on_timeout, test).
Capturing only one stall may show only one surface — **both can be live.**

### CODE-PROVABLE half (do this FIRST — it is deterministic and fabrication-immune)
The relay arm-E starvation does NOT need the flaky repro to confirm: read the `select!`
(raw_proxy.rs:419-455). When `!streams.is_empty()`, `client_wait == upstream_wait == 2 ms`;
with `biased`, the two sleep arms become ready simultaneously and arm-D (client) is polled
before arm-E (upstream) ⇒ `upstream.conn.on_timeout()` is unreachable on a tie. PROVE this
two ways and report BOTH: (i) the code argument above; (ii) a counter in the instrumented
build — `arm_e_fires` vs `arm_d_fires` since start. If `arm_e_fires == 0` (or ≪ arm_d)
across a whole run while streams were in flight, the starvation is CONFIRMED empirically,
**independent of whether this test's particular stall was triggered by the backend or the
relay.** This is the single most important number to capture.

### The DISCRIMINATOR to capture at each stall (so each stall is classified)
For the stuck `sid`, record on BOTH sides:
- Backend per-stream: `peer_fin_seen` (did the request FIN ARRIVE at the backend?),
  `our_fin_sent`, `stream_finished(sid)`, queued-bytes-left; backend `on_timeout`-call
  count and `path_stats.total_pto_count`; backend `stats()` sent_bytes vs acked_bytes.
- Relay upstream leg: `path_stats` (sent/lost/retrans/total_pto_count/cwnd/sent_bytes) +
  `stats().acked_bytes`, `stream_finished(sid)`, plus `arm_e_fires`/`arm_d_fires`.
- → If backend `peer_fin_seen=false` for the stuck sid ⇒ the relay's REQUEST FIN never
  arrived ⇒ **relay surface (#2, production)**. If backend `peer_fin_seen=true` but its
  response tail is un-acked with PTO flat ⇒ **backend surface (#1, test)**. Tabulate the
  classification over ALL stalls in the burst.

### Decision tree ADDENDUM
Outcome **5. BOTH surfaces live** is allowed and likely: report the per-stall
classification counts. A BOTH outcome means the Phase-2 fix is two-part (production:
relay services `upstream.conn.on_timeout()` reliably; test: backend services `on_timeout`
independent of recv) — and per R12 the production half is single-sourced + the matrix/Mode
A relay-adjacent code is checked for the same `biased`-tie-starvation pattern.

### Capture MULTIPLE stalls
Run the burst long enough to capture **≥3 distinct stalls** (the 22 % rate over ≥50 iters
gives ~11). Classify each. One capture is not the mechanism — the distribution is.
</content>
</invoke>
