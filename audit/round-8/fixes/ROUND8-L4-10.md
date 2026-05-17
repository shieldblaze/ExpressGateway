# Plan for ROUND8-L4-10 — Commit verifier-log baselines per kernel (5.15 / 6.1 / 6.6); fix `scripts/verify-xdp.sh` diff-gate

Finding-ref:     ROUND8-L4-10 (high, Open — audit-of-audit)
Reference:       kernel-selftests research bundle ("the kernel's own regression tests for the BPF subsystem assert exactly which error the verifier returned"); Round-7 retrospective; `audit/ebpf/round-2-review.md:619-624` (EBPF-2-07 marked Verified-Fixed at `ffde98c`).
Coverage-gap:    Theme 1 — verbatim. EBPF-2-07 was the canonical example: the script existed; the artefact (committed `.log.committed`) did not; the diff-gate is dormant. Bundle B-2 with OPS-09 (div-ops owns the doc-lint audit-of-audit half; this plan owns the verifier-log capture half).

Files touched:
  - `scripts/verify-xdp.sh`                          (normalise to `--kernel <kver>` flag form; pin lvh-images digest; fail-loud when `.log.committed` absent; add `bpftool prog test-run` correctness step)
  - `audit/ebpf/verifier-logs/5.15.log.committed`    (NEW — captured baseline)
  - `audit/ebpf/verifier-logs/6.1.log.committed`     (NEW — captured baseline)
  - `audit/ebpf/verifier-logs/6.6.log.committed`     (NEW — captured baseline)
  - `audit/ebpf/verifier-logs/README.md`             (document the capture/refresh procedure; link OPS-09's doc-lint contract)
  - `audit/ebpf/round-2-review.md`                   (re-classify EBPF-2-07 to `Verified-Fixed-Partial` while baselines are absent; flip to `Verified-Fixed` once the three logs land + the script hard-fails on absence)
  - `audit/unsafe-justifications.md:109`             (correct the over-claim about "verifier-log diff gate in CI")
  - `.github/workflows/ci.yml` or equivalent         (add three matrix jobs `verify-xdp-5.15 / 6.1 / 6.6`; pin lvh-images digest)

Approach:

1. **Script: `scripts/verify-xdp.sh`**:
   - Change CLI from positional `verify-xdp.sh 5.15` to flag form
     `verify-xdp.sh --kernel 5.15` (and keep positional as a
     deprecated alias for one release with a WARN line). The flag
     form lets us add `--update-baseline` later for the refresh
     workflow.
   - Pin `IMAGE`: replace the floating
     `quay.io/lvh-images/kernel-images:${KVER}-main` with a
     digest-pinned ref. The first CI green run captures the digest;
     a top-of-file `LVH_IMAGE_*` const table stores them.
   - Fail loud when `.log.committed` is absent:
     ```bash
     if [ ! -f "${OUT_LOG}.committed" ]; then
         say "FATAL: no committed baseline at ${OUT_LOG}.committed"
         say "either:  1) commit ${OUT_LOG} as the baseline (first-time setup)"
         say "         2) restore the deleted committed baseline (regression)"
         exit 2
     fi
     ```
     Replace the current `if [ -f "${OUT_LOG}.committed" ]; then diff ... fi`
     pattern (which silently passes when the file is absent).
   - **Correctness step**: after the verifier-log capture, run
     `bpftool prog test-run /sys/fs/bpf/probe data_in <synthetic.pkt>`
     and assert the dst MAC byte at offset 0 was rewritten as
     expected. This catches the case where the program *loads* but
     does not *execute the rewrite path* — same shape as
     ROUND8-L4-05's silent-drop probe, but here it's compile-time
     correctness on the BPF object.
   - Exit codes:
     - `0` = log matches baseline
     - `1` = log drift from baseline (intentional or accidental)
     - `2` = missing baseline (developer must commit it)
     - `3` = environment problem (docker missing, image pull fail)

2. **Capture the baselines**:
   - Procedure (one-shot, in a privileged CI runner with docker +
     KVM):
     - `scripts/build-xdp.sh` produces `crates/lb-l4-xdp/src/lb_xdp.bin`.
     - `scripts/verify-xdp.sh --kernel 5.15 --update-baseline`
       (new flag): captures the log, writes
       `audit/ebpf/verifier-logs/5.15.log.committed`. Same for 6.1 and 6.6.
     - The lvh-images digest is captured into the script's pin
       table.
   - Commit the three baselines + the digest pin + the script
     changes in a single review-able PR.

3. **README rewrite** (`audit/ebpf/verifier-logs/README.md`):
   - Replace the current text with:
     - "What lives here: per-kernel verifier-log snapshots
       (`5.15.log.committed`, `6.1.log.committed`, `6.6.log.committed`)."
     - "Refresh procedure: `scripts/verify-xdp.sh --kernel <K> --update-baseline`,
       then `git add audit/ebpf/verifier-logs/<K>.log.committed`. Include the
       refresh in the same PR as the source change that motivated
       the diff."
     - "Audit-of-audit hook: `doc-lint.sh` (per Bundle B-2,
       OPS-09) cross-checks that every `Verified-Fixed(<sha>)`
       claim in `audit/ebpf/round-2-review.md` referencing a
       `verifier-log` artefact has the artefact at the referenced
       sha."

4. **Audit-register update** (`audit/ebpf/round-2-review.md:619-624`):
   - EBPF-2-07 row status changes from `Verified-Fixed(ffde98c)` to
     `Verified-Fixed-Partial(ffde98c, <baseline-sha>)` while only
     two of three kernels' baselines are committed; flip to
     `Verified-Fixed(<baseline-sha>)` once all three are landed.

5. **Truth in `unsafe-justifications.md`**:
   - Line 109's claim ("validated by the kernel verifier on every
     load and additionally pinned via the verifier-log diff gate in
     CI") becomes accurate only once the baselines land. Update the
     line to reference the specific baseline shas, or leave it as-is
     and lay the patch beneath the baseline-commit so the doc and
     reality agree at the same commit.

6. **CI matrix** (workflow file):
   - Three jobs: `verify-xdp-5.15`, `verify-xdp-6.1`, `verify-xdp-6.6`.
   - Each runs:
     ```
     scripts/build-xdp.sh
     scripts/verify-xdp.sh --kernel <K>
     ```
   - Job fails if:
     - log drift from baseline (exit 1) — operator may need to refresh
     - baseline missing (exit 2) — never should happen post-commit
     - bpftool test-run rewrite assertion fails (new — see step 1)
   - All three jobs gating on every PR that touches
     `crates/lb-l4-xdp/ebpf/**` or `crates/lb-l4-xdp/src/**`.

7. **Bundle peer (OPS-09)**:
   - OPS-09 owns `doc-lint.sh` audit-of-audit extension: every
     `Verified-Fixed(<sha>)` claim must have a matching committed
     artefact at the referenced sha; the audit-of-audit walks the
     register and asserts the artefact's *content* (not just
     existence) matches the recommendation text in the row.
   - The contract between OPS-09 and this plan:
     - This plan produces the three `.log.committed` artefacts.
     - OPS-09's doc-lint regex includes `audit/ebpf/verifier-logs/.*\.log\.committed`
       under the "EBPF-2-07 Verified-Fixed requires" table.
     - Both plans land together; div-ops sequences after div-l4.

Proof:

- After the patch: `scripts/verify-xdp.sh --kernel 6.1` exits 0 in
  CI on the baseline commit; exits 1 on the next BPF source change
  until the operator runs `--update-baseline` and re-commits.
- `audit/ebpf/verifier-logs/` contains three `.log.committed`
  files plus an updated README.
- `audit/ebpf/round-2-review.md` EBPF-2-07 row shows the new sha.
- The CI matrix shows three green `verify-xdp-*` jobs on the
  baseline-commit PR.

Risk / blast radius:

- Capturing the baselines requires a *one-time* privileged CI run
  (or local docker w/ KVM); if the runner is misconfigured, the
  baseline is wrong. Mitigation: the OPS-09 doc-lint sanity-checks
  the recorded `bpf_log_level` line in the log; if it's empty,
  doc-lint flags it.
- Future kernel-version drift (6.6 -> 6.10) means the matrix needs
  to grow. Operationally: bump the matrix on every LTS release.
  Procedure documented in the README.
- The `bpftool test-run` correctness assertion uses a hand-built
  synthetic packet; the construction is in `scripts/verify-xdp.sh`
  as a heredoc. Keep the packet small and stable.

Cross-ref:
- Bundle B-2 peer: OPS-09 (doc-lint audit-of-audit).
- ROUND8-L4-09 (`ptr_at` checked-add): the verifier-log baseline
  will include the `llvm.uadd.with.overflow` lowering — both
  plans coordinate on the baseline commit.
- ROUND8-L4-01, -02, -03, -04, -07, -08: any change to the BPF
  source forces a baseline refresh; sequence those plans to land
  baselines together.

Owner:           div-l4 (this half); div-ops (doc-lint half via OPS-09)
Lead-approval: approved 2026-05-14 team-lead-r8
