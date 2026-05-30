# SESSION 18 — handoff (post-S17)

> S17 closed **PARTIAL**. It did NOT land a fix and did NOT promote Mode B — by
> design. CF-S16-RELAY-STALL was characterized to a sharp, reproduced class and the
> leading candidate fix was **refuted by controlled experiment**, then escalated R6
> genuinely-large (owner ruling 2026-05-30). The win this session is a *correct
> refutation* instead of a plausible-but-wrong fix — the opposite of the S16
> fabrication. Start S18 with FRESH judgment, not a sixth in-loop guess.

## Start here
1. `git checkout feature/quic-proxy-s17` (tip = the S17 PARTIAL close; base
   `feature/quic-proxy-s16 @ b1e9e621`, NOT promoted to main). Source == the verified
   S16 tree; only `audit/` docs were added in S17 (no code delta).
2. Read `audit/quic/s17-report.md` §Phase 1 (the proven class + the five refuted
   hypotheses) and `audit/quic/s17-logs/p1-verifier-findings.md` (the decisive
   client-leg capture). Do NOT re-derive; do NOT retry any of the five.
3. Confirm base tip, kill strays, `cargo test --workspace --all-features` ×3 to
   re-confirm the Phase-0 baseline (non-Mode-B green ×3; Mode B suite non-deterministic
   via the stall). `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` exported per-command.

---

## PRIORITY 1 — CF-S16-RELAY-STALL (R6 genuinely-large) — the gate to Mode B

### Proven CLASS (reproduced 6/6 in S17; do not re-litigate)
A CLIENT-LEG lost-tail loss-recovery failure on the LB→client path. On a stall: the
relay reads the whole backend response and reclaims the stream (`u2c.done`,
`up_stream_finished=true` all sids); **exactly one** upstream→client stream is short at
the client by a VARYING amount (`short_by` 396–44458 B, sid 4/8/16 vary = lost-flight
signature); the relay's `params.conn` **never retransmits** the lost tail
(`CL-PATH total_pto=0 retrans=0 stream_retrans_bytes=0` on every stall) and the LB
client conn idle-closes at 20 s (`break_reason=client_closed`). The backend's frozen
`acked_bytes` is a downstream symptom, not the trigger.

### FIVE refuted hypotheses — do NOT retry any
1. tick/wake-cadence (S16, 6 variants). 2. test-driver single-datagram drain (S16).
3. wake-latency `.min(IDLE_TICK)` cap (S16). 4. backend-PTO-never-fires (S17 diag — the
backend PTO DID fire + retransmit). 5. **relay arm-E `upstream.conn.on_timeout()`
starvation (S17 verifier — REFUTED by intervention: forcing both `on_timeout`s every
iteration left stalls at baseline 4/75, client leg still `total_pto=0`).** Arm-E
starvation is real but is NOT this stall's cause.

### THE OPEN QUESTION (S18's target — one focused pass)
*Why does quiche's `params.conn` neither arm a PTO nor retransmit a lost, already-
`stream_send`-accepted client-leg tail on a relay-FIN'd stream?* The S17 capture is
missing the numbers that distinguish the three sub-causes. Capture, AT a stall, on
`params.conn` for the short sid:
- `params.conn` **`bytes_in_flight`** + **`cwnd`** (is bytes_in_flight ≥ cwnd → cwnd-
  blocked? S17 saw cwnd vary 13500–82350 — capture it WITH bytes_in_flight).
- the **stream send-buffer length** (bytes accepted by `stream_send` but NOT yet put on
  the wire) — i.e. is the tail BUFFERED-NOT-SENT vs SENT-AND-LOST?
- quiche's **`loss_detection_timer` state** (None vs a future Instant) — is a PTO armed
  at all, and for when?
- whether `params.conn.send()` returns `Done` because of cwnd/pacing/anti-amplification
  vs genuinely-nothing-to-send.
This DEFINITIVELY splits the root into:
  (a) **buffered-not-sent**: send-side cwnd/pacing deadlock (bytes_in_flight pinned ≥
      cwnd by unacked in-flight packets, no PTO probe) → fix is on the relay's client-
      leg send/pacing, NOT retransmission;
  (b) **in-flight-unacked-no-PTO**: a genuine quiche loss-recovery timer not arming for
      the tail → likely a quiche-version interaction; consider a minimal quiche-only
      repro (two loopback conns, last flight dropped) to confirm independent of the
      relay;
  (c) **sent-lost-ACK-detected-retransmit-suppressed**: a stream-state bug where the
      relay/quiche won't retransmit a FIN'd stream's tail.
Then READ quiche `recovery.rs` `set_loss_detection_timer` / `on_loss_detection_timeout`
/ the PTO-arming predicate against the captured state. Only after the root is pinned to
(a)/(b)/(c) does a builder write a fix — verified R13 (a)+(b)+(c) with the burst (the
stall rate is noisy 6–22%; use ≥75 iters) and a load-bearing negative control.
NOTE: the test client is REAL quiche (driven by a hand-rolled one-recv-per-iter pump);
the no-retransmit is a `params.conn` (production relay) property independent of client
pump cadence — do not re-blame the driver (#2 above).

### Evidence (in-repo)
`audit/quic/s17-logs/`: `p1-diag-eng-findings.md`, `p1-verifier-findings.md`,
`p1-diag-burst-50.log`, `p1-verifier-clientdump-60.log`, `p1-verifier-probe-75.log`.

## PRIORITY 2 — CF-S17-B3-VERIFY-DONE-UNWRAP (tractable test fix)
`crates/lb-quic/tests/s16_b3_reset_propagation_verify.rs:627` does
`client.stream_send(SIBLING_STREAM, SIBLING_PAYLOAD, true).unwrap()`; quiche returns
`Err(Done)` transiently (stream-grant / conn-flow-control not yet available right after
the FWD_STREAM reset) → fast panic under the saturated ×3 gate (~1/3 of runs in S17
Phase 0). Test-only, client-local (no relay round-trip). Fix: pump + retry the sibling
open (as the test already does for recv) instead of unwrapping. Author≠verifier.

## Mode B status (do NOT promote until the stall is closed)
- B1/B2/B3: BUILT + quiet-box-verified (S16). Production relay correctness (byte-
  identical, backpressure, cancellation) is NOT in question — only liveness under the
  stall. The whole Mode B WIRE suite (`s16_b2_multistream`, `s16_b2_backpressure`
  resume, etc.) is non-deterministic under the open stall.
- B4 (RawDatagramRelay + bounded queue + drop-newest), B5 (bounded-state flood), B6
  (`lb/src/main.rs` wiring + `quic_modeb_*` metrics + full wire test + two-connections
  proof + 0-RTT-rejection proof): **NOT STARTED** (s16-plan §4). Native QUIC proxy is
  NOT complete until B4–B6 land AND the stall is closed.

## Lead recommendation on S18 scope (owner asked)
**Give the stall its OWN focused diagnostic session (S18 = close the stall).** Reasons:
(1) the chaos/soak suite cannot meaningfully soak Mode B while it stalls 6–22%;
(2) the stall is the gate to native-QUIC-complete, a prerequisite for the soak suite's
Mode B coverage; (3) the stall needs deep quiche loss-recovery focus — a different mode
of work from building a soak harness, and mixing them invites the end-of-budget-pressure
risk the owner flagged. Sequence: **S18 = stall (then B4–B6 if it closes early) →
S19 = chaos/soak suite.** If parallelism is wanted, the soak suite *could* begin on the
already-stable surface (Mode A passthrough, the 9 matrix cells, H3 termination, XDP)
independent of Mode B — but keep the stall diagnosis single-focus.

## Program-level remaining (oldest carry-forward — the highest-leverage prod-readiness item)
THE CHAOS/SOAK SUITE (does not yet exist). Plus WebSockets-over-H2 (RFC 8441) + H3
(RFC 9220), gRPC-over-H3 conformance, full h3spec. Carry-forwards unchanged: CF-DEP-1
(Dependabot, owner, 11 sessions), CF-IGN-1 (16 inherited `#[ignore]`), CF-FCAP-MARGIN,
F-ESC-1 (multi-kernel CI lane), N-1 (jumbo-MTU), Mode A perf tiers (io_uring v1.1,
XDP v1.2), CF-S16-RELAY-GENERIC-ERR (LOW). Hygiene note: 5 stale `git stash` entries
from OLD sessions (s7/verifier, prod-readiness/round-4) pollute the shared repo — a
latent R9 hazard; a future session owning those branches should drop them.
