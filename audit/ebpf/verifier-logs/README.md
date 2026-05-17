# Verifier-log snapshots

ROUND8-L4-10 captures the BPF verifier's output per supported LTS
kernel. CI runs `scripts/verify-xdp.sh --kernel <KVER>` for each of:

- `5.15.log.committed` — current LTS floor (`DEPLOYMENT.md` Kernel floor).
- `6.1.log.committed`  — current LTS.
- `6.6.log.committed`  — current rolling LTS.
- `7.0.log.committed`  — REAL baseline captured live on the audit
  foundation-pass box (F-ESC-1): aya `ProgramInfo` kernel
  verifier-derived counters + `bpftool prog show` on the loaded prog
  id. This box runs kernel **7.0** which is OUTSIDE the official
  5.15/6.1/6.6 matrix.

### Kernel support window (F-DOC-1 / auditor-3 A-3)

The aya-ebpf program uses **no BPF ring buffer and no kfuncs**, and
its BTF/CO-RE relocations load cleanly, so the **effective verifier
floor is ~5.15 LTS**. 5.15/6.1/6.6 are the officially-validated LTS
window. Kernel **7.0 is validated live** (D-1 native ENA attach +
this directory's real `7.0.log.committed`) but is **outside the
official matrix**.

> **OPEN R7 PRODUCT DECISION (not gate-blocking, not asterisked):**
> whether to formally **extend the official verifier-baseline matrix
> to 7.x** (and declare 7.x officially supported) is a product
> decision for the owner — it is recorded here, not decided in the
> audit. 7.0 itself is proven working; this item is only about
> declaring/CI-pinning the wider 7.x window.

The committed baselines are the load-bearing audit artefact: the
script hard-fails (exit 2) if a baseline is absent, and hard-fails
(exit 1) on any drift from the baseline. This closes the no-op
diff-gate posture EBPF-2-07's first attempt (`ffde98c`) shipped.

## Refresh procedure

1. After a BPF source change in `crates/lb-l4-xdp/ebpf/src/`,
   rebuild the ELF: `scripts/build-xdp.sh`.
2. Run each kernel matrix entry in `--update-baseline` mode:
   ```
   scripts/verify-xdp.sh --kernel 5.15 --update-baseline
   scripts/verify-xdp.sh --kernel 6.1  --update-baseline
   scripts/verify-xdp.sh --kernel 6.6  --update-baseline
   ```
   This rewrites the three `.log.committed` files in place.
3. Eyeball the diffs (`git diff audit/ebpf/verifier-logs/`).
   Look for new "speculative", "unbounded", or "state explosion"
   strings — those are the bad signals.
4. `git add audit/ebpf/verifier-logs/*.log.committed` and include
   the refresh in the same PR as the source change that motivated
   it.

## Local reproduction (without CI)

`scripts/verify-xdp.sh` needs `--privileged` Docker plus lvh-images
to boot per-kernel VMs. Most developers will rely on the CI matrix.

The script will refuse to run with the floating lvh-images tag
unless `EG_ALLOW_FLOATING_IMAGE=1` is set (reproducibility). CI
runs use the pinned-digest path; see the `LVH_IMAGE_*` table at the
top of the script.

## Current state (this commit)

The three `.log.committed` files in this directory carry the
`HARNESS-CAPTURED-PENDING-CI-RERUN` marker — they are placeholders
emitted because the worktree used to author this fix did not have
`bpf-linker` available to rebuild the BPF ELF, nor a privileged
runner to actually exercise the kernel matrix. The first green CI
run after this commit lands MUST refresh all three files.

The script's exit semantics enforce this: it returns exit 1 (drift)
on every CI run until the baselines are real. That is the desired
audit-of-audit posture — "script committed but baseline absent" is
EXACTLY the failure mode ROUND8-L4-10 closes.

## Audit-of-audit hook

`scripts/ci/doc-lint.sh` (OPS-09's half of bundle B-2) cross-checks
that every `Verified-Fixed(<sha>)` claim in
`audit/ebpf/round-2-review.md` referencing a `verifier-log`
artefact has the artefact at the referenced sha with content
matching the recommendation. The check covers this directory.

Owner: `ebpf` / `div-l4` (snapshot capture); `rel` / `div-ops` owns
the CI image pin and the matrix workflow file (`OPS-09`).
