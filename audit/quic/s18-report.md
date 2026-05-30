# SESSION 18 — report (CF-S16-RELAY-STALL → finish Mode B → promote)

Branch `feature/quic-proxy-s18` off `feature/quic-proxy-s17 @ 2c5d2f0b` (S17 PARTIAL tip,
NOT promoted). Box: 8 cores. `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` per-command.
Standing rules R1–R15 in force; R15 (no result from an incomplete job) load-bearing.

---

## Phase 0 — baseline + hygiene  (IN PROGRESS)

### Hygiene
- Base tip confirmed `2c5d2f0b`; branch `feature/quic-proxy-s18` created + pushed.
- `ps aux`: no cargo/rustc/quic strays from S17 (only this session's claude proc). No
  leftover worktrees (only the main checkout). Disk: 36 GB free on `/` (≥25 GB ✔).
- Stray branches noted (not this session's to delete): `worktree-agent-aae742917af3c81c2`,
  5 stale `git stash` entries (R9 hazard, carried — owner of those branches drops them).

### Lead system re-grounding (read, not re-derived)
- quiche 0.28 `congestion/recovery.rs::set_loss_detection_timer` clears the loss timer when
  `bytes_in_flight.is_zero() && peer_verified_address` → "no PTO armed" ⟺ bytes_in_flight==0.
- `run_dual_pump`: `RELAY_TICK` short-wait clamp gated on `!streams.is_empty()`; after
  reclaim the client leg parks on `params.conn.timeout()` (~20s idle when no loss timer).
- **The test client driver never calls `client_conn.on_timeout()`** → a lost client→relay
  control frame (ACK / MAX_STREAM_DATA) is never retransmitted by the client. S17's probe
  never touched the client. → primary falsifiable target for Phase 1.

### Findings tracked into the gate (R4)
- CF-S16-RELAY-STALL (BLOCKER, OPEN) → Phase 1 three-bucket diagnosis.
- CF-S17-B3-VERIFY-DONE-UNWRAP (test fragility) → **FIXED** @ `90ac0b8b` (builder-1):
  the bare `stream_send(SIBLING_STREAM,..,true).unwrap()` at :625 replaced with a bounded
  (10 s) pump+retry loop (cursor over `SIBLING_PAYLOAD`, FIN rides the final fully-accepted
  chunk, `Err(Done)`=no-op, pumps `flush`+`try_recv_one` between tries). Test-only, no
  assertion weakened, no `#[ignore]`, Cargo.lock unchanged. NOTE: the file is gated
  `#![cfg(feature = "test-gauges")]` → only runs under `--all-features` / `--features
  test-gauges` (silent `0 tests` otherwise) — the R1 baseline uses `--all-features` so it
  is covered. Lead independently reviewed the diff (author≠verifier); independent
  verification burst deferred to the quiet box (Phase 3) to avoid perturbing diag-eng's
  timing-sensitive diagnosis. builder-1 self-test (read from completed output): 10× ×4
  single-threaded + 15× saturated headline = 25/25 PASS.

_(baseline ×3 + test-fix evidence appended as they complete — every claim cites a completed log.)_

---

## Phase 1 — three-bucket diagnosis  ✅ ROOT PROVEN (diag-eng + lead code-confirm)
**Bucket (a)-RELAY-DROPS-BUFFERED-TAIL** — relay-side production bug in
`raw_proxy.rs::pump_dir`. Read gate not gated on `src_fin_seen`; after the source FIN is read
(quiche collects the stream) a short-write leaves a tail in `half.pending`; the next turn
re-reads the collected stream → `Err(InvalidStreamState)` → generic arm `pending.clear();
done=true` DROPS the tail + never forwards the FIN. Measured both paths: `dropping_pending ==
short_by` 4/4; only the InvalidStreamState arm fires; `send_off_back == client got_len` 7/7
(ZERO wire loss); `bytes_in_flight=0`, `loss_timer=None` (explains S17 `total_pto=0`). Koch:
client `on_timeout` REFUTED (12/90 ≈ baseline); read gate `!src_fin_seen` ELIMINATES (0/90 +
0/75). Findings: `s18-logs/p1-diag-findings.md` (10 cited evidence logs). REFUTES S17's
lost-flight framing AND the brief's test-harness hypothesis — the bytes were DELETED by the
relay, not lost on the wire. Lead independently re-verified the mechanism in source.

## Phase 2 — stall fix + B4/B5/B6  (fix landed; independent verify RUNNING)
**FIX @ `1414d656` (builder-1):** `pump_dir` read gate → `while !half.src_fin_seen &&
half.pending.len() < STREAM_RELAY_WINDOW`. One condition; B3 reset/stop arms untouched;
single-sourced (fixes both relay legs, R12). + deterministic regression test
`post_fin_short_write_reread_does_not_drop_tail` (drives pump_dir directly; completes BOTH
sides of the src bidi so quiche genuinely collects it → the real InvalidStreamState re-read;
byte-exact + FIN asserts). **Negative control PROVEN load-bearing** (builder, read from
completed output): with fix PASS; one-line reverted → FAILS (`pending=0, done=true,
fin_sent=false` = tail dropped); restored PASS. Self-check: 20/20 `s16_b2_multistream`
isolation (no 90 s stall). Only raw_proxy.rs changed; Cargo.lock unchanged. Lead diff-review
PASS (author≠verifier).
**Independent verifier (DONE — `s18-logs/p2-verifier-findings.md`, lead spot-checked BURST_DONE
lines):** root CONFIRMED + fix VERIFIED to R13.
- R13(b) isolation burst BOTH paths: FIXED `1414d656` **0/90 multistream + 0/75 backpressure**
  (own worktree/target, quiet box). PRE-FIX `a231b761` 9/90 + 4/75.
- R13(c) load-bearing negative control: same harness 9/90→0/90, 4/75→0/75; + deterministic
  regression test PASS-fixed/FAIL-prefix on a one-line toggle.
- Root independently reproduced: every drop (10/10) = `InvalidStreamState` arm, `src_fin_seen=true`,
  `dropping_pending == short_by` EXACT; write-error arm never fired.
- No-regression: all 5 Mode B wire tests + lb-quic lib (68) pass under `--features test-gauges`.

**⇒ CF-S16-RELAY-STALL is CLOSED** (proven + independently confirmed + fixed + R13 b/c verified;
R13(a) ×3 gate = Phase 3). **B4/B5/B6:** decision at Phase-3-green (R7 honest-stop available:
stall-closed + clean gate beats rushed B4-B6).

## Phase 3 — gates  ✅ GREEN ×3 (promote decision pending)
Gate tree `81187de6` (fix `1414d656` in tree; only audit/ docs changed after → gate reflects
the promotable source). Logs `/home/ubuntu/Code/eg-s18-gate/`, summary archived
`s18-logs/p3-gate-summary.txt`. Each run read to completion + independent FAILED grep (empty).

| gate | result |
|---|---|
| `cargo fmt --check` | exit 0 ✔ |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 ✔ |
| `cargo test --workspace --all-features` run 1 | **1364 passed / 0 failed / 18 ignored** (230 bins) |
| run 2 | **1364 / 0 / 18** |
| run 3 | **1364 / 0 / 18** |

Deterministic ×3, no asterisk (R1). The previously non-deterministic Mode B stall tests
(`s16_b2_multistream`, `s16_b2_backpressure` resume) now pass inside the ×3 parallel gate
= **R13(a)** confirmed. No regression (R3): S1–S15, 9 matrix cells, H3 termination, Mode A
passthrough, XDP, foundation, security, Mode B B1/B2/B3 all green ×3. 18 ignored = inherited
CF-IGN-1 carry-forward. The gate also re-verifies CF-S17-B3 (s16_b3 ran ×3 under `--all-features`
at 8-core saturation — exactly where it flaked — and passed).

**R13 for the stall fix — ALL THREE satisfied:** (a) ×3 parallel gate clean [this section];
(b) ≥75-iter dual-path isolation burst ZERO stalls [verifier 0/90 + 0/75]; (c) load-bearing
negative control [verifier 9/90→0/90, 4/75→0/75 + deterministic regression test fail-prefix/
pass-fixed].

**Coverage (scoped):** the production change is a single added condition (`!half.src_fin_seen`)
on an already-heavily-exercised line in `pump_dir`; the changed branch is covered by the new
deterministic regression test `post_fin_short_write_reread_does_not_drop_tail` (proven
load-bearing) + the full Mode B wire suite ×3. No new uncovered production code → session
sub-metric met by construction (a separate instrumented llvm-cov run for a one-condition change
is disproportionate and disk-risky at 13 GB free; documented rather than run).

## Promote (R7 honest-stop — OWNER-APPROVED)
Owner ruling (2026-05-30): promote the stall-closed state now → SESSION 18 COMPLETE; B4/B5/B6
get a fresh session with full budget (the exact "stall-closed + clean gate beats rushed B4-B6"
case R7 was written for; B6's 0-RTT/two-connections security proofs must not be rushed at
end-of-budget — same pressure that produced the S16 fabrication). Promoted `feature/quic-proxy-s18`
→ `main` `--no-ff`. main was at S15 `30cc22f2` (Mode A only); this promote brings the full Mode B
**core relay** (S16 B1/B2/B3 built → S17 diagnosed → S18 stall-fixed) — the first Mode B promote.
S19 handoff: `audit/quic/s19-handoff.md` (B4/B5/B6 then the chaos/soak suite; stall mechanism
recorded for the soak to watch the loss-recovery path under sustained load).

## VERDICT: **SESSION 18 COMPLETE** — CF-S16-RELAY-STALL CLOSED + R13-verified; stall-closed
Mode B core relay (B1/B2/B3) promoted to main.
- **Stall:** root PROVEN (relay-side `pump_dir` post-FIN re-read drop), independently confirmed
  (diag-eng + verifier + lead code-read), FIXED (`1414d656`), R13 (a)+(b)+(c) all satisfied on
  BOTH affected paths (multistream + backpressure resume), zero stalls post-fix.
- **Refutations (integrity):** S17's "lost client-leg tail / no PTO" framing and the brief's
  "test client never runs loss recovery" hypothesis were BOTH refuted by measurement (Koch:
  client `on_timeout` 12/90 ≈ baseline) — the bytes were DELETED by the relay, not lost.
- **Gates:** R1 ×3 GREEN deterministic (1364/0/18), clippy + fmt clean, no regression (R3).
- **Test fix:** CF-S17-B3-VERIFY-DONE-UNWRAP fixed + verified.
- **Mode B B4/B5/B6:** NOT started (honest-stop → S19). Native QUIC NOT complete; NOT
  production ready (no chaos/soak suite — S19).
- **Integrity:** zero fabrications; every gate/burst/repro cites a completed log read to end
  (R15); author≠verifier on every increment; tree pristine between agents; promote owner-approved.
