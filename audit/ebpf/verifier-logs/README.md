# Verifier-log snapshots

ROUND8-L4-10 captures the BPF verifier's output per supported LTS
kernel. CI runs `scripts/verify-xdp.sh --kernel <KVER>` for each of:

- `5.15.log.committed` — current LTS floor (`DEPLOYMENT.md §27`).
- `6.1.log.committed`  — current LTS.
- `6.6.log.committed`  — current rolling LTS.

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
