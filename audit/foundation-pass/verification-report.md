# FOUNDATION AUDIT — Independent Verification Report (Phase 3)

verifier (authored NONE of the fixes; author != verifier strict, R5).
Branch `audit/foundation-pass` @ HEAD `33f8c6a4`. Box c6a.2xlarge 8c,
kernel 7.0.0-1004-aws, ens5 = `ena` (firmware-version EMPTY — confirmed
via `ethtool -i ens5`).

Every defect mechanism was independently re-derived from source; every
regression independently demonstrated to FAIL on the defect (parent /
reverted-hunk scratch worktree) and PASS at HEAD. No builder prose
trusted.

---

## PART A — per-finding verdicts

| Finding | Commit | Verdict |
|---|---|---|
| F-COR-2 / BL-1 | 469052ec | **VERIFIED-FIXED** |
| F-COR-3 | 20e22560 | **VERIFIED-FIXED** |
| F-COR-4 | 05d801c1 | **VERIFIED-FIXED** |
| F-COR-5 | 366be028 | **VERIFIED-FIXED** |
| F-COR-8 | e42657f6 | **VERIFIED-FIXED** |
| F-COR-7 | a7b6aacf | **VERIFIED-FIXED** |
| F-DOC-1 | 3e23ba9e | **VERIFIED-FIXED** |
| F-ESC-1 | 59946e21 | **VERIFIED-FIXED** (per disposition) |
| F-SEC-1 | 044db8da | **VERIFIED-FIXED** |
| F-COR-1 | 9e41d07f | **VERIFIED-FIXED** |
| F-COR-6 | 7770de99 | **VERIFIED-FIXED** |

### F-COR-2 / BL-1 (R1 blocker) — VERIFIED-FIXED
Re-derived from source: `BackendState::active_connections()` =
single `AtomicU64::load(Relaxed)` (cannot tear); `inc_connections` =
`fetch_add(1, AcqRel)`; `dec_connections` = saturating CAS loop;
`Backend::sync_from_state` = single `.load()` copy. Product correct.
With 4 inc + 4 dec threads the counter is genuinely non-monotonic, so
the deleted `snapshot ∈ [min(pre,post),max(pre,post)]` 3-non-atomic-read
bracket is provably unsound (the `pre==post` captured counter-example
is decisive: a single AtomicU64 load cannot tear, so the out-of-bracket
value was real). Sound post-join exact-equality check RETAINED unchanged
(diff lines 95-106). New `snapshot_tracks_atomic_monotonic_midflight`
is deterministic.
- HEAD: 4/4 pass ×3 deterministically.
- Independent negative: stale-by-one stub injected into
  `sync_from_state` (scratch worktree) →
  `snapshot_tracks_atomic_monotonic_midflight FAILED — snapshot != atomic
  at iter 1: snapshot=0, atomic=1`. The new sub-test genuinely fails on
  the defect it guards. Not test-weakening (R5): a provably-false
  assertion removed, real coverage preserved + strengthened.

### F-COR-3 — VERIFIED-FIXED
Mechanism: 3 tests used fixed shared temp paths
(`eg-test-zero-drop`/`eg-drain-h2`/`eg-drain-h3`) → path-collision
class (R2 REAL DEFECT). Fix routes all three through
`unique_temp_dir` (pid + nanos + process-global monotonic `SEQ`
counter — collision-free by construction). Soak test uses
`drain::unique_temp_dir("reload-soak")` (line 68); H2/H3 drain tests
use `unique_temp_dir("drain-h2"/"drain-h3")` (969/1111).
- HEAD: full `reload_zero_drop` binary 6/6 pass, 0 ignored.

### F-COR-4 — VERIFIED-FIXED
Mechanism: pre-fix `let _ = clean_eof;` then unconditional `FinOnly`
→ a byte-complete-but-never-FIN-closed socket was misclassified a
clean PASS; the test could not fail on the close-defect it guards.
Fix: pure `classify_close()` requires observed `Ok(0)` for `FinOnly`,
else `BodyCompleteNoClose` (hard fail). No product source changed
(latent).
- HEAD: 6/6 (incl. truth-table + deterministic stub negative
  regression).
- Independent negative: reverted `classify_close` to pre-fix
  `let _ = clean_eof; FinOnly` (scratch) →
  `classify_close_requires_observed_fin_for_finonly FAILED
  (left Some(FinOnly), right Some(BodyCompleteNoClose))` AND
  `negative_regression_complete_body_never_closed_is_caught FAILED`
  (same signature). Real assertion strengthened, not weakened.

### F-COR-5 — VERIFIED-FIXED
`proptest` is an unconditional dev-dep (`crates/lb-h3/Cargo.toml:17`);
the empty `proptest = []` feature (line 21) gated nothing, so
`#![cfg(feature="proptest")]` was dead.
- Parent (366be028~1): default `cargo test -p lb-h3 --test
  proptest_qpack` → `running 0 tests`.
- HEAD: `running 4 tests` / 4 passed (default AND --all-features).
Mirrors S1 sibling `25d8ad84`.

### F-COR-8 — VERIFIED-FIXED
Latent hardening: per-test `static CERT_SEQ: AtomicU64` `-{seq}`
suffix mirroring round8:75-81 makes cert paths unique by
construction, closing the latent parallel-collision class structurally
(scope is the structural fix; no pre-fix failure exists by design —
single-test file). HEAD: 1/1 pass.

### F-COR-7 — VERIFIED-FIXED
- `classify()` is byte-UNCHANGED: the ONLY `-` lines in the
  `nic_compat.rs` diff are the dead fail-open arm
  `Err(_) => return Ok(DrvSupport::Allowed)`. New pure
  `classify_unresolved_firmware(driver, kernel)` used only on the
  firmware-unresolved branch.
- Real box: `ethtool -i ens5` → driver `ena`, kernel 7.0,
  firmware-version EMPTY → firmware unresolved. Integration test
  `drv_supported_ens5_is_allowed_on_this_not_known_bad_ena_box` ran
  the FULL real `drv_supported("ens5")` path (NO SKIP line in stdout)
  → Allowed. Native XDP NOT regressed (D-1 consistent).
- Dead-path-now-live: pre-fix the firmware-unresolved arm returned
  `Ok(Allowed)` unconditionally so a pre-6.7 ena could never Refuse;
  post-fix it routes through `classify_unresolved_firmware`, which the
  unit test proves Refuses ena/{6.6,6.1,5.15,5.10} (ROUND8-L4-05
  cited) and Allows ena/{7.0,6.7-boundary}.
- HEAD: lib 13/13 (incl. `classify_unchanged_is_pure_and_untouched`,
  `ena_unresolved_fw_modern_kernel_stays_allowed`,
  `ena_unresolved_fw_prebad_kernel_refuses`), integration 1/1.

### F-DOC-1 — VERIFIED-FIXED
Docs-only. DEPLOYMENT.md kernel floor aligned to ~5.15 effective;
5.15/6.1/6.6 validated window; 7.0 live-validated (D-1); 7.x = open
R7 product decision recorded (not gate-blocking, not asterisked). No
code/script change. Consistent with finding disposition.

### F-ESC-1 — VERIFIED-FIXED (per disposition)
7.0.log.committed is a REAL capture: prog_id 78, prog_tag
0x72c34ab7e4f44914, verified_insns 9284, xlated 12800, jited 7264 +
bpftool JSON (3 real data lines; the "HARNESS-CAPTURED-PENDING" string
appears ONLY in an explanatory comment saying what it is NOT).
5.15/6.1/6.6 remain pure placeholders (0 real data lines) and are
honestly escalated via R7(a) commit `799c098d` + verbatim
infeasibility message (no /dev/kvm, no nested virt, empty digests).
Not asterisked, not falsely claimed — matches disposition exactly.

### F-SEC-1 (SECURITY, highest scrutiny) — VERIFIED-FIXED
(a) **Revised mechanism is correct.** `close()` on a TCP socket with
unread RX → kernel RST (Linux `tcp_close` / RFC 1122 §4.2.2.13), not
FIN. Independent socket-level repro (`/tmp/rst-repro`) demonstrated
the RST itself (client saw `GOAWAY-SIGNAL<<ECONNRESET>>` in the
no-drain case) and that the drain-then-shutdown analog yields a clean
close with the payload intact. The wire-level GOAWAY discard is
timing/buffer-dependent (exactly why auditor-2 saw it 6/48 only under
starvation — a scheduler race), which correctly justifies a structural
unit gate over a flaky wire assertion.
(b) **Client actually receives the GOAWAY.** `CleanCloseIo` is wired
into the PRODUCTION H2 server connection (`h2_proxy.rs:639`
`builder.serve_connection(TokioIo::new(CleanCloseIo::new(io)), svc)`).
The live corroboration test asserts `saw_goaway`, where `ServerGoAway`
requires the h2 CLIENT to surface `e.is_remote() || e.is_go_away()`
(the client's h2 stack decoded the GOAWAY frame) — a real wire
confirmation, not merely a unit assertion. Corroboration PASSES
post-fix.
(c) **Unit gate is a faithful structural proxy.** The `Probe` sets
`shutdown_called_with_undrained = true` iff inner `poll_shutdown` (the
FIN) is reached while `delivered < to_deliver` (unread inbound) —
exactly the RST-on-unread-close precondition. The gate asserts that is
`false`, EOF seen, and all 200 KiB drained before FIN. Independent
negative: short-circuited the drain (`if false && !self.drained`,
scratch) → `clean_close_io_drains_inbound_before_fin FAILED:
"F-SEC-1: CleanCloseIo did not drain inbound to EOF before FIN"` and
`clean_close_io_drain_is_bounded FAILED`. The gate genuinely fails on
the defect it guards — a faithful proxy, not a weaker substitute.
(d) Original `tests/h2_security_live.rs::rapid_reset_goaway` still
PASSES (full binary 6/6, no #[ignore]). Corroboration test NOT
#[ignore]d.
(e) **DRAIN_CAP cannot be abused.** Hard cap `DRAIN_CAP = 256 KiB` via
`drain_budget` (decremented per read, loop breaks at 0);
`Poll::Pending → break` (never waits on a silent/slow client). No
unbounded read, no DoS hang. `clean_close_io_drain_is_bounded` proves
an endless inbound source still terminates teardown ≤ DRAIN_CAP.
HEAD: unit gate 2/2, corroboration 1/1, h2_security_live 6/6.

### F-COR-1 (highest scrutiny) — VERIFIED-FIXED
Re-derived: in `H2Proxy::proxy_request` (`h2_proxy.rs:1199`)
`http_body_util::Limited::new(body, MAX_REQUEST_BODY_BYTES)` +
`.collect().await` (line 1225) drives hyper/h2 protocol validation to
completion; cap-exceed → `BodyTooLarge`→413 (1234); protocol/IO error
→ `BadRequest`→400 (1236); pseudo-trailer → `BadRequest`→400 (1253) —
ALL strictly BEFORE `self.pool.acquire_async(backend_addr)` (line
1267, the dial). The race is closed STRUCTURALLY (no scheduler
dependence). 64 MiB named cap bounded via `Limited` (no unbounded
read). H3: `feed_body` InTrailers arm rejects pseudo-headers (RFC 9114
§4.3 → Err → Reset), unit-proven both directions.
- HEAD: `h2_validation_before_forward` 3/3.
- Independent negative: moved the dial BEFORE `collect()` (pre-fix
  streamed ordering, scratch) →
  `content_length_mismatch_never_leaks_backend_body FAILED` and
  `pseudo_header_in_trailers_never_leaks_backend_body FAILED`
  ("F-COR-1 DEFECT: backend was dialed for a … malformed request").
  Dial-count gate is a real scheduler-independent guard. The 413 cap
  test correctly stayed green (orthogonal to dial ordering).
- Valid-trailer e2e all green: trailer_passthrough 8/8,
  h3_h1_trailers_resp_e2e 2/2, bridging_h2_h1/h2_h2/h2_h3/h3_h1 1/1
  each, h2_proxy_e2e 3/3, lb-quic feed_body unit 2/2.

No working test was #[ignore]d, deleted, or had a real assertion
gutted (R5). F-COR-2 and F-COR-4 corrections replaced provably-unsound
/ provably-blind assertions and were independently proven to still
fail on the real defect.

---

## PART B — Phase 3 R1 re-baseline

### `cargo fmt --check` — **FAIL** (verbatim: phase3/fmt.txt)
NOT clean at HEAD. 7 COMMITTED builder files require reformatting:
```
Diff in crates/lb-l4-xdp/src/nic_compat.rs:384
Diff in crates/lb-l4-xdp/tests/round8_ena_kernel_blocklist.rs:22, :57
Diff in crates/lb-l4-xdp/tests/round8_verifier_baseline_70.rs:25, :73
Diff in tests/balancer_counter_sync.rs:82
Diff in tests/h2_rapid_reset_goaway_under_load.rs:247
Diff in tests/h2_validation_before_forward.rs:203
Diff in tests/reload_zero_drop.rs:1277, :1290
```
Working tree had NO local modifications (only untracked phase3/) — this
is the committed state. Mechanism (R2, REAL DEFECT not environmental):
pure rustfmt nonconformance, deterministic, reproducible. Spans
F-COR-7, F-COR-2, F-SEC-1, F-COR-1, F-COR-4 commits. The builders'
"fmt clean" checkpoint claims are FALSE.

### `cargo clippy --all-targets --all-features -- -D warnings` — **FAIL** (verbatim: phase3/clippy.txt)
NOT clean at HEAD (exit 101). 6 deny-level errors in committed builder
test code:
```
error: doc list item without indentation  tests/reload_zero_drop.rs:1194,1195,1196,1197  (F-COR-4)
error: this `map_or` can be simplified     tests/h2_rapid_reset_goaway_under_load.rs:247,259  (F-SEC-1)
error: could not compile `lb-integration-tests` (test "reload_zero_drop") due to 4 previous errors
error: could not compile `lb-integration-tests` (test "h2_rapid_reset_goaway_under_load") due to 2 previous errors
```
Mechanism (R2, REAL DEFECT): deterministic clippy lint failures under
`-D warnings`. The builders' "clippy clean" checkpoint claims are
FALSE.

### `cargo test --workspace --all-features -- --test-threads=8` ×5
(verbatim: phase3/test-run-{1..5}.txt) — **PASS, 5/5 deterministic**

```
run 1: 1094 passed; 0 failed; 16 ignored | 201 test binaries all "result: ok" | 0 non-ok result lines
run 2: 1094 passed; 0 failed; 16 ignored | 201 test binaries all "result: ok" | 0 non-ok result lines
run 3: 1094 passed; 0 failed; 16 ignored | 201 test binaries all "result: ok" | 0 non-ok result lines
run 4: 1094 passed; 0 failed; 16 ignored | 201 test binaries all "result: ok" | 0 non-ok result lines
run 5: 1094 passed; 0 failed; 16 ignored | 201 test binaries all "result: ok" | 0 non-ok result lines
```
ALL 5 passed (R1 minimum 3; did 5 for robustness given BL-1's prior
1/3 rate — the F-COR-2 fix holds deterministically). The 16 ignored
are PRE-EXISTING privileged/environment-gated tests (live XDP
`drv_mode_attach_to_ens5`, `capture_real_70_verifier_baseline`
[needs sudo], `h2spec_server_conformance_passes` [documented
corroboration-only per F-COR-1 disposition], bpffs, etc.). NONE is a
finding-regression test. Every finding regression test was
independently confirmed to RUN and PASS, and to FAIL on its defect
(Part A). No working test #[ignore]d/deleted/gutted by the fixes (R5).
The test workspace is GREEN; the R1 RED verdict is due solely to the
fmt + clippy static gates.

---

## R1 VERDICT: **NOT BASELINE-GREEN**

Two R1 static gates FAIL deterministically at HEAD (fmt + clippy), in
committed builder code, with proven non-environmental mechanism. Per
R1 ("No test excluded, no asterisk, no 'green except'") the Phase-3
baseline is RED regardless of the 5× test result.

ROUTING: the fmt + clippy nonconformance goes back to a builder (lead
routes). Affected commits: F-COR-7 (a7b6aacf), F-COR-2 (469052ec),
F-SEC-1 (044db8da), F-COR-1 (9e41d07f), F-COR-4 (05d801c1). The
underlying defect FIXES are all independently VERIFIED-FIXED (Part A);
the failure is purely fmt/lint hygiene on the committed test+source
files — a focused, tractable re-touch (run `cargo fmt`; fix 6 clippy
lints), then re-baseline.
