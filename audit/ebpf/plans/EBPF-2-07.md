# Plan for EBPF-2-07 — Verifier-log matrix: 5.15 / 6.1 / 6.6 captured in CI
Finding-ref:     EBPF-2-07 (medium, Open)
Files touched:
  - `audit/ebpf/verifier-logs/5.15.log`        (NEW — committed snapshot)
  - `audit/ebpf/verifier-logs/6.1.log`         (NEW)
  - `audit/ebpf/verifier-logs/6.6.log`         (NEW)
  - `scripts/verify-xdp.sh`                    (NEW — driver that boots a per-kernel image, loads with `BPF_LOG_LEVEL=2`, captures log)
  - `.github/workflows/xdp-verify.yml`         (NEW — matrix CI job; or a section in an existing CI file if rel already wired one)
  - `crates/lb-l4-xdp/src/loader.rs`           (expose `verifier_log_level` and a capture hook for tests)

Approach:

1. **Capture mechanism**. aya's `EbpfLoader::verifier_log_level(LogLevel::Verbose2)`
   sets the kernel's `bpf_attr.log_level = 2`, which tells the kernel
   to emit the full verifier trace. aya allocates a 16 MiB log buffer
   by default — sufficient for our 8 KiB program. The log surfaces
   as `EbpfLoader::load()` returns; aya stores it in
   `Object::verifier_log` accessible via a public method (or via the
   error variant on rejection).

2. **Per-kernel runners**. Three options, ordered by preference:

   a. **GitHub Actions `vmlinux` container approach**: pull
      `ghcr.io/cilium/little-vm-helper` or
      `quay.io/lvh-images/kind:6.6-main`, mount the workspace,
      run `cargo test -p lb-l4-xdp --test verifier_snapshot
      -- --ignored`. lvh has prebuilt LTS kernels for 5.10, 5.15,
      6.1, 6.6 and is what Cilium uses.

   b. **`vng` (virtme-ng)**: lightweight microVM that boots a host
      kernel binary in 1-2 s. Use upstream kernel.org `linux-*-lts`
      tags built once and cached in CI.

   c. **`bpftool prog load -k <vmlinux>`**: load against precise
      kernel headers without booting a VM. This only checks
      BTF-aware verification; for full verifier coverage we need
      an actual running kernel. **Rejected** for that reason.

   Option (a) selected — easiest CI wire-up; lvh maintains the
   kernel images so we don't.

3. **Driver script `scripts/verify-xdp.sh`**:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   KERNEL_VERSION="${1:?usage: verify-xdp.sh <5.15|6.1|6.6>}"
   IMAGE="quay.io/lvh-images/kernel-images:${KERNEL_VERSION}-main"
   OUT="audit/ebpf/verifier-logs/${KERNEL_VERSION}.log"
   docker run --rm --privileged \
     -v "$PWD:/work" -w /work \
     "$IMAGE" \
     bash -c '
       cargo test -p lb-l4-xdp --test verifier_snapshot --features capture-verifier -- --ignored 2>&1 \
         | sed -n "/^---BEGIN VERIFIER LOG---$/,/^---END VERIFIER LOG---$/p"
     ' > "${OUT}.new"
   # Normalise: strip instruction-pointer absolutes (kernel reloc churn) but
   # keep relative offsets and decision counts.
   sed -E -e 's/0x[0-9a-f]{16}/0xADDR/g' \
          -e 's/processed [0-9]+ insns/processed N insns/' \
          -e 's/peak_states [0-9]+/peak_states N/' \
       "${OUT}.new" > "$OUT"
   rm "${OUT}.new"
   diff -u "audit/ebpf/verifier-logs/${KERNEL_VERSION}.log" "$OUT" || {
     echo "VERIFIER LOG DIFF on kernel ${KERNEL_VERSION}";
     exit 1;
   }
   ```
   The normalisation step removes spurious diff noise (kernel
   address relocation, exact insn count) but keeps the structural
   verifier output — branch decisions, state-pruning hits,
   register-bound tightening notes.

4. **Test plumbing**. New integration test
   `crates/lb-l4-xdp/tests/verifier_snapshot.rs`:
   - Gated on feature `capture-verifier` so dev builds skip it.
   - Loads `lb_xdp.bin` with `verifier_log_level(LogLevel::Verbose2)`.
   - Prints `---BEGIN VERIFIER LOG---\n{log}\n---END VERIFIER LOG---`
     to stdout (the driver script greps for these sentinels).
   - Marks `#[ignore]` — only runs under `--ignored`.

5. **CI matrix entry** (in `.github/workflows/xdp-verify.yml`):
   ```yaml
   strategy:
     matrix:
       kernel: ["5.15", "6.1", "6.6"]
   steps:
     - uses: actions/checkout@v4
     - run: cargo install bpf-linker --locked
     - run: scripts/build-xdp.sh
     - run: scripts/verify-xdp.sh ${{ matrix.kernel }}
   ```
   `bpf-linker` install is shared with EBPF-2-01's CI requirement.

6. **Tripwire rule**. Any PR that changes
   `crates/lb-l4-xdp/ebpf/src/*.rs` MUST also update the three
   logs. CI fails otherwise. Reviewers eyeball the diff to confirm
   no new verifier complaints (e.g. "speculative" / "unbounded"
   warnings) appear.

Proof:

- Test name: `lb-l4-xdp/tests/verifier_snapshot.rs::verifier_log_matches_committed_snapshot`
- Invariants:
  1. The program loads (exits 0 from `EbpfLoader::load`).
  2. The captured log, after the normalisation sed pass, matches
     `audit/ebpf/verifier-logs/<kver>.log` byte-for-byte.
  3. Negative-control: a `#[cfg(feature = "neg-control-bad-bounds")]`
     build that deliberately omits a bounds check MUST cause
     `load()` to fail with a verifier-rejection error AND the
     captured log MUST contain `"R<n> unbounded memory access"`
     or similar verifier diagnostic — proves the capture path
     surfaces real failures, not just successes.
- Runs as the CI matrix above.

Risk / blast radius:

- **Log normalisation hides real changes**. The sed strip is
  intentionally narrow (only addresses and counter values). If it
  ever masks a meaningful diff, the reviewer-eyeball step catches
  it; the worst case is a forced regen with no real change, which
  is a benign noise cost.
- **lvh kernel images stop being maintained**. Mitigation: pin
  the lvh image digest in the workflow; only bump deliberately.
  If lvh disappears we fall back to `vng` against kernel.org
  tags (option 2-b above).
- **bpf-linker install in CI is slow** (~30 s). Mitigation: GHA
  cache `~/.cargo/bin/bpf-linker`.

Cross-ref:
- EBPF-2-01: BTF emission makes verifier logs much more readable
  (type names instead of bare offsets).
- All other EBPF findings: the verifier matrix is the
  regression-detection net for any of them.
- rel kernel-floor decision (round-2 cross-review §A-4): if rel
  lowers the floor below 5.15 in their Round-2 file, add the
  extra kernel to the matrix.

Owner:          ebpf
Lead-approval: approved 2026-05-13 team-lead
