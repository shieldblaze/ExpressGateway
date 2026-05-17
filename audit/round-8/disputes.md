# Round 8 — Disputes against prior verdicts

Findings where the prior audit examined and dismissed the area and
Round-8 disagrees. Lead arbitration required.

---

### ROUND8-L4-10 — EBPF-2-07 `Verified-Fixed` is materially incomplete

**Prior coverage**
- Finding: EBPF-2-07 (`audit/ebpf/round-2-review.md`).
- Status: `Verified-Fixed(ffde98c) — rel round-5 sign-off; script + README in place, supported kernels (5.15 / 6.1 / 6.6) documented, bash -n clean, --help exits 64 with usage.`
- Carried into `audit/STATE` Round-7 closure and `audit/FINAL_REPORT.md` as a green box.
- The audit-of-audit gate (REL-2-01 / REL-2-14 doc-lint) did not check that the artefact the gate references actually exists.

**Round-8 view**
- `git show ffde98c --stat` returns exactly:
  - `audit/ebpf/verifier-logs/README.md  | 30 ++++++++++`
  - `scripts/verify-xdp.sh                | 109 +++++++++++++++++++++++++`
- `ls audit/ebpf/verifier-logs/` returns only `README.md`. No `.log` snapshots are committed.
- `scripts/verify-xdp.sh:111` checks `if [ -f "${OUT_LOG}.committed" ]; then diff ... ; fi`. The conditional is always false; the diff-gate is a permanent no-op.
- `audit/unsafe-justifications.md:109` claims the gate "additionally pinned via the verifier-log diff gate in CI" — false.
- The `rel` round-5 verifier signed off on `bash -n` cleanliness + `--help` exit code + README existence. That is verification of *the script itself*, not of *the verification having happened*. Round-5 verification text uses "script + README in place" — accurate as far as it goes; insufficient for the EBPF-2-07 contract which required per-kernel log capture.

**Suggested verdict**
- Re-classify EBPF-2-07 to `Verified-Fixed-Partial` (script harness landed; baselines NOT landed) or roll back to `Open`. Phase-D plan must commit per-kernel `.log.committed` snapshots before the next round closes.
- Update `audit/unsafe-justifications.md:109` to remove the false claim, OR land the snapshots in the same commit that asserts it.
- Bundle with ROUND8-OPS-09 (doc-lint audit-of-audit gate) so the next "Verified-Fixed(<sha>) — script present" claim has its referenced artefact existence validated.

**Round-8 stance**: this is the canonical "audit-of-audit" finding for the prior register's self-grading; preserving it as DISPUTE ensures lead arbitration rather than silently downgrading. Without the snapshots, every other EBPF finding in the register that depended on "verifier-log gate" as proof is exposed (notably ROUND8-L4-09 ptr_at overflow guard, which the gate was supposed to detect at compile time for aya #1562-class issues).
