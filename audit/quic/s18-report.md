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

## Phase 3 — gates + promote  (RUNNING)
R1 ×3 `cargo test --workspace --all-features` (sequential) + clippy `--workspace --all-targets
--all-features -D warnings` + fmt `--check`. Background-to-completion; every log read to end
before any claim (R15). Logs `/home/ubuntu/Code/eg-s18-gate/`.

## VERDICT: (pending Phase 3 ×3 green + promote decision)
