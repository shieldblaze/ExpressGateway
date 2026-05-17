# builder-2 → lead : PLAN APPROVAL REQUEST (tasks #10–#14)

Consolidated plan written: `audit/foundation-pass/plans/builder-2-plan.md`.

Covers all 5 owned items with approach / files / test design / regression
proof / risk per finding:

1. **F-COR-2 / BL-1** (#10, R1 BLOCKER): delete the provably-unsound
   concurrent bracket assertion in `tests/balancer_counter_sync.rs`,
   keep the existing sound post-join exact-equality, ADD a
   single-threaded monotonic mid-flight sub-test. Unsoundness proof
   recorded (snapshot=8650 bracket=[8649,8649], pre==post — single
   AtomicU64 cannot tear; bidirectional divergence = oscillation).
   Product unchanged. Reproduce false-fail pre / 0-fail post under 24×
   CPU oversubscription ≥80 runs.
2. **F-COR-3 + F-COR-4** (#11, serialized, `reload_zero_drop.rs`, 2
   commits): H2/H3/reload-soak per-iter `unique_temp_dir` (note:
   reload-soak is at file scope, will call `drain::unique_temp_dir`);
   gate FinOnly on observed Ok(0), add `BodyCompleteNoClose` hard-fail
   + negative regression (stub holds socket open → must FAIL).
3. **F-COR-5 + F-COR-8** (#12, 2 commits): remove dead
   `#![cfg(feature="proptest")]` in `lb-h3/tests/proptest_qpack.rs`
   (mirror S1 25d8ad84); add `CERT_SEQ` AtomicU64 to
   `h3_graceful_close.rs` (mirror round8).
4. **F-COR-7** (#13): fail-CLOSED in `drv_supported` when a
   blocklisted/fragile driver (ena) has unresolved/empty firmware;
   real `drv_supported("ens5")` full-path regression test (this box is
   ena → runs for real; pre-fix Allowed=FAIL, post-fix Refuse=PASS).
5. **F-DOC-1 + F-ESC-1** (#14): align DEPLOYMENT.md + verifier-logs
   README to ~5.15 effective floor, declare 7.x status, record the
   "extend matrix to 7.x" R7 product decision as open. Capture a REAL
   7.0 verifier baseline via the proven aya load path + kernel
   `ProgramInfo` (verified_insns / xlated / jited / tag) +
   `bpftool prog show` on the loaded id (bpftool can inspect but NOT
   load the legacy-map ELF — auditor-3 tooling note confirmed).

**F-ESC-1 environment finding (pre-flagged):** this box has NO lvh, NO
qemu, NO vng — only Docker. `scripts/verify-xdp.sh` needs privileged
Docker + lvh-images whose digests are literal PLACEHOLDERS in-script.
5.15/6.1/6.6 are very likely genuinely infeasible here; I will ATTEMPT,
and if so capture the WHY verbatim and message you to escalate the
residual multi-kernel CI lane per R6/R7(a) (NOT asterisked, R4).

bpftool DOES NOT load this ELF (libbpf rejects legacy `maps` section) —
aya is the only working loader (already re-verified: d1 attach PASS on
7.0 just now, prog_id=62, STATS 0→10). 7.0 real capture IS tractable.

**Blocking on your "approved" before ANY source edit (R5).**
