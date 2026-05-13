# Verifier-log snapshots

EBPF-2-07 captures the BPF verifier's output per supported LTS
kernel. CI runs `scripts/verify-xdp.sh <KVER>` for each of:

- `5.15.log` — current LTS floor (`DEPLOYMENT.md §27`).
- `6.1.log`  — current LTS.
- `6.6.log`  — current rolling LTS.

The first CI run after EBPF-2-01's rebuilt ELF lands writes these
files as the baseline snapshots. Subsequent runs diff against the
committed copies and fail on drift; reviewers eyeball the diff to
confirm no new verifier complaints (e.g. "speculative",
"unbounded", "state explosion") appear.

The driver normalises out absolute kernel addresses, instruction
counts, and per-state-search counters so the structural verifier
output (branch decisions, state-pruning hits, register-bound
tightening notes) is the load-bearing diff signal.

## Local reproduction

```
scripts/build-xdp.sh                      # rebuilds lb_xdp.bin
scripts/verify-xdp.sh 6.1                  # one kernel at a time
diff -u 6.1.log.committed 6.1.log         # confirm structural match
```

Owner: `ebpf` (snapshot capture); `rel` owns the CI image pin and
the matrix workflow file.
