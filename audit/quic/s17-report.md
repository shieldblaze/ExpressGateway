# SESSION 17 — report (CF-S16-RELAY-STALL → finish Mode B → promote)

Branch `feature/quic-proxy-s17` off `feature/quic-proxy-s16 @ b1e9e621`.
Box: 8 cores (stall is scheduling-sensitive — diagnosed/verified at production core count).

Standing rules in force: R1 (×3 green), R2 (mechanism-classify every failure),
R5 (author≠verifier), R13 (layered timing verification), **R15 (no result from an
incomplete job — every claim cites a completed, read-to-end log)**.

---

## Phase 0 — baseline + hygiene  ✅ COMPLETE

### Hygiene
- Base tip confirmed: `feature/quic-proxy-s17 @ b1e9e621` (S16 tip, NOT promoted — correct).
- `ps aux`: no cargo/rustc strays, no S16 heartbeat cron, no disk-guard, no leftover
  worktrees, no root-owned artifacts. Only the system `cron -f` daemon + this session.
- Disk: stale S16 build cache cleaned (`eg-target/debug`, 28 GB) → **51 GB free** (≥25 GB ✔).
- Quarantined a prior incomplete-pass's `s17-logs/*` artifacts (07:06–07:18) — NOT results
  (R15) — then removed them.

### Gates (each foreground/background-to-completion, read to end — R15; logs in `s17-logs/`)
| gate | command | result | log |
|---|---|---|---|
| compile | `cargo test --workspace --all-features --no-run` | **exit 0**, no warnings | `p0_compile.log` |
| fmt | `cargo fmt --check` | **exit 0** clean | `p0_fmt.log` |
| clippy | `cargo clippy --all-targets --all-features -- -D warnings` | **exit 0** clean | `p0_clippy.log` |

### R1 ×3 baseline — `cargo test --workspace --all-features` (sequential ×3)
Logs `p0_test_run{1,2,3}.log`. Per-binary Running→FAILED pairing (authoritative):

| run | failed binary | failing test | mechanism | class |
|---|---|---|---|---|
| 1 | `s16_b2_backpressure` | `…client_throttled_then_complete_on_resume` | `Elapsed(())` after **90 s** waiting for `done_rx` (resumed transfer never completes) — `s16_b2_backpressure.rs:788` | **LIVENESS STALL** (= CF-S16-RELAY-STALL class) |
| 2 | `s16_b3_reset_propagation_verify` | `forward_client_reset_propagates_with_code_conn_stays_up` | `unwrap()` on `Err(Done)` after **0.34 s** — the **client's own** `stream_send(SIBLING_STREAM,…)` at `:627` | **TEST-HARNESS FLAKE** (fast; distinct) |
| 3 | — (all pass) | — | — | — |

**Crucial reframe (R2 mechanism-classified):**
- **Everything OUTSIDE the Mode B `s16_*` suite is green across all 3 runs** — S1–S15,
  the 9 matrix cells, H3 termination, Mode A passthrough, XDP datapath, foundation,
  security. **No regression (R3) anywhere outside Mode B.** (sanity grep: "all FAILED
  binaries are `s16_*`".)
- **CF-S16-RELAY-STALL is NOT confined to `s16_b2_multistream`.** It is a *general* Mode B
  relay-liveness defect: this round it stalled the **backpressure-resume** path
  (`s16_b2_backpressure`, 90 s Elapsed) while `s16_b2_multistream` passed ×3 (its ~22% did
  not fire in 3 samples — expected). The author's own comment at `:784` ("sub-second
  isolated") confirms 90 s is a NEVER-completes, not a slow-completes. **This makes the
  stall squarely a production liveness bug** (backpressure-resume is core relay behaviour),
  not a property of one test.

### Findings opened at baseline
- **CF-S16-RELAY-STALL** (BLOCKER, OPEN) — general Mode B relay liveness stall. → Phase 1.
- **CF-S17-B3-VERIFY-DONE-UNWRAP** (test fragility, OPEN) — `s16_b3_reset_propagation_verify.rs:627`
  does `client.stream_send(SIBLING_STREAM, SIBLING_PAYLOAD, true).unwrap()`; quiche may
  return `Err(Done)` transiently (stream-grant / connection-flow-control not yet available
  right after the FWD_STREAM reset). FAST fail, client-local (no relay round-trip involved)
  ⇒ NOT the stall, NOT a production defect. Tractable test-only fix (pump+retry the sibling
  open, as the test already does for recv). Will be fixed in Phase 2 with author≠verifier.
  Per R4 it is tracked into the gate, never asterisked.

_Phase 0 verdict: baseline established; non-Mode-B workspace clean ×3; Mode B suite
non-deterministic via one liveness stall (general) + one fast test flake. Proceed to
Phase 1 diagnosis (HARD CAP: one focused pass)._

---

## Phase 1 — CF-S16-RELAY-STALL diagnosis  ⛔ ESCALATED R6 (genuinely-large)

Two independent, author≠verifier passes (diag-eng, then the independent verifier),
each with its own env-gated instrumentation in its own throwaway worktree, each burst
run FOREGROUND and read to completion (R15). The lead confirmed every cited number
against the completed logs. Evidence preserved in-repo:
`audit/quic/s17-logs/p1-diag-eng-findings.md`, `p1-verifier-findings.md`,
`p1-diag-burst-50.log`, `p1-verifier-clientdump-60.log`, `p1-verifier-probe-75.log`.

### What was PROVEN (mechanism pinned to a CLASS, reproduced 6/6)
On a stall, **exactly one upstream→client stream loses a tail flight on the LB→client
path, and the relay's client-facing `params.conn` never retransmits it**:
1. The request reaches the backend fully (`peer_fin_seen=true`, all sids) and the
   backend echoes + FINs every stream (`our_fin_sent`/`stream_finished=true`,
   `queued_left=0`) — NOT a relay request-FIN loss.
2. The relay reads the **entire** backend response and **reclaims** its stream table
   (`u2c.done`, `up_stream_finished=true` all sids) — NOT a stuck read, NOT a u2c hang.
3. The short fall is on the **CLIENT leg**: one stream is short at the client by a
   VARYING amount (`short_by` 396–44458 B; sid 4/8/16 vary run-to-run = a lost-flight
   signature, not a logic bug). `got_fin=false`, `client_readable=false` (the client
   drained all the LB delivered and waits for bytes that never arrive).
4. The relay's `params.conn` shows the lost tail **never retransmitted**:
   `CL-PATH … total_pto=0 retrans=0 stream_retrans_bytes=0` on EVERY stall, while
   `cl_sent_bytes > cl_acked_bytes`. quiche either detects the loss (`lost>0`) or holds
   the tail unacked-in-flight (`lost=0`) but **never arms a PTO** for it.
5. The LB client conn idle-closes at the 20 s `max_idle_timeout`
   (`break_reason=client_closed`) → the test hits its 25 s budget wall.
The backend's frozen `acked_bytes` (diag-eng's headline) is a downstream SYMPTOM (the
relay has no new ack-eliciting data for the backend once the stream is done), not the
trigger.

### What was REFUTED (do NOT retry — controlled experiment)
The lead's pre-designed fix and diag-eng's implicated surface — **"service
`upstream.conn.on_timeout()` (arm-E) unconditionally"** — was tested by direct
intervention (probe forcing BOTH `on_timeout`s every iteration, loop spinning hot,
forced upstream PTOs visible `UP-PATH total_pto=2..5`): **stalls PERSISTED at baseline
(4/75 ≈ 5.3% vs 11/185 ≈ 5.9%)**, client leg STILL `total_pto=0 retrans=0`
(`p1-verifier-probe-75.log`). The client-leg PTO is **never armed**, so forcing
`on_timeout` cannot help. This ALSO refutes the "loop parks too long after reclaim"
theory (the probe loop did NOT park and still stalled). **Arm-E starvation is REAL
(code-provable; `arm_e` fired 1/150 + 0/185) but it is NOT this stall's mechanism.**

Disproven/refuted hypotheses now total FIVE across S16+S17: (1) tick/wake-cadence,
(2) test-driver single-datagram drain, (3) wake-latency `.min(IDLE_TICK)` cap,
(4) backend-PTO-never-fires surface (backend PTO DID fire + retransmit), (5) relay
arm-E `on_timeout` starvation (refuted by intervention).

### Why R6 genuinely-large (per the HARD CAP + owner ruling 2026-05-30)
The CLASS is pinned and independently reproduced, but the EXACT root — *why quiche's
`params.conn` neither arms a PTO nor retransmits an already-`stream_send`-accepted,
lost client-leg tail on a relay-FIN'd stream* — is a deep third-party (quiche)
loss-recovery question, not isolated in the capped pass, and the brief's candidate fix
is refuted by experiment. Per exit condition (c) and the owner's explicit ruling, this
is escalated R6 genuinely-large; **Mode B is NOT promoted** (an unverified, subtle
library-internals fix landed under end-of-budget pressure is the highest-risk
"appears-fixed-but-isn't" outcome — declined after S16). Next-session direction in the
S18 handoff.

### Second finding (Phase 0) — CF-S17-B3-VERIFY-DONE-UNWRAP
Tracked, not asterisked (documented + reproduced + scoped). Test-only fragility in
`s16_b3_reset_propagation_verify.rs:627` (`client.stream_send(SIBLING_STREAM,…).unwrap()`
on transient quiche `Done`). Carried to S18 with the Mode-B-suite stabilization (it is
part of the same "Mode B wire tests are non-deterministic" picture). Tractable test
fix: pump+retry the sibling open instead of unwrapping.

---

## Phase 2 / Phase 3 — NOT REACHED (gated behind the stall)
- Stall fix: NOT attempted (cause not isolated; candidate refuted — R6 escalated).
- B4 (datagram relay) / B5 (bounded-state flood) / B6 (main.rs wiring + 0-RTT proof):
  **NOT STARTED** — blocked behind the stall (s16-plan §4), per exit condition (c).
- No source changed this session: `feature/quic-proxy-s17` source == the verified S16
  tree (`b1e9e621`); only `audit/` docs were added. The Phase 0 ×3 baseline IS the
  close gate (non-Mode-B green ×3; Mode B suite non-deterministic via the open stall).
  No re-gate needed (no code delta). Mode A / matrix / H3 / XDP intact (R3).

---

## VERDICT: SESSION 17 PARTIAL — CF-S16-RELAY-STALL escalated R6; Mode B unpromoted

- CF-S16-RELAY-STALL: **mechanism characterized to a sharp class + reproduced 6/6, NOT
  isolated to a root; leading candidate fix REFUTED by controlled experiment →
  R6 genuinely-large, escalated.** Honest handoff in `audit/quic/s18-handoff.md`.
- Mode B B1/B2/B3 remain BUILT + quiet-box-verified (S16) but the suite is
  non-deterministic under the open stall → **NOT promoted** (R1/R3).
- B4/B5/B6 not started. Native QUIC proxy is NOT complete this session.
- Integrity: zero fabrications; the candidate fix was refuted by experiment, not
  shipped; every result cites a completed log (R15); tree pristine; no promote.
