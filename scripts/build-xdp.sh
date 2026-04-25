#!/usr/bin/env bash
# Pillar 4a + 4b-1: build the standalone eBPF crate into a loadable BPF ELF.
#
# Two toolchains:
#   - workspace:  stable 1.85 (pinned by root rust-toolchain.toml)
#   - lb-xdp-ebpf:  nightly pinned by crates/lb-l4-xdp/ebpf/rust-toolchain.toml,
#                   required by aya-ebpf + bpf-linker.
#
# On success the ELF (~3 KiB for the 301-line Pillar 4a program) is copied to
# crates/lb-l4-xdp/src/lb_xdp.bin. crates/lb-l4-xdp/build.rs detects it and
# emits cfg(lb_xdp_elf) so lb_l4_xdp::LB_XDP_ELF becomes available in
# dependent crates and integration tests.
#
# Pillar 4b-1: this script is now a HARD dependency for updating the checked-in
# ELF. If bpf-linker is absent the script still exits 0 after logging the
# remediation — cached ELF remains in place, so downstream builds keep
# working from the committed artifact.

set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$(pwd)"
EBPF_DIR="${REPO_ROOT}/crates/lb-l4-xdp/ebpf"
OUT_BIN="${REPO_ROOT}/crates/lb-l4-xdp/src/lb_xdp.bin"

say() { printf 'build-xdp.sh: %s\n' "$*"; }

# Extract the nightly channel pinned by the ebpf crate.
if [ ! -f "${EBPF_DIR}/rust-toolchain.toml" ]; then
  say "Missing ${EBPF_DIR}/rust-toolchain.toml; cannot determine nightly pin."
  exit 1
fi
NIGHTLY=$(awk -F'"' '/^channel[[:space:]]*=/ { print $2 }' "${EBPF_DIR}/rust-toolchain.toml")
if [ -z "${NIGHTLY}" ]; then
  say "Could not parse channel from ${EBPF_DIR}/rust-toolchain.toml; aborting."
  exit 1
fi
say "ebpf crate pinned to rustc ${NIGHTLY}"

# Make sure rustup has the pinned nightly with rust-src + the BPF target.
if ! rustup toolchain list 2>/dev/null | grep -q "^${NIGHTLY}\|^nightly"; then
  say "Installing ${NIGHTLY} toolchain…"
  rustup toolchain install "${NIGHTLY}" --component rust-src --target bpfel-unknown-none || {
    say "rustup toolchain install ${NIGHTLY} failed. Skipping ELF build."
    exit 0
  }
else
  # Ensure rust-src is present on the pinned channel.
  rustup component add rust-src --toolchain "${NIGHTLY}" >/dev/null 2>&1 || true
  rustup target add bpfel-unknown-none --toolchain "${NIGHTLY}" >/dev/null 2>&1 || true
fi

# bpf-linker must match rustc's LLVM major; install with the pinned nightly
# so its transitive MSRV (1.88+ at time of writing) is satisfied.
if ! command -v bpf-linker >/dev/null 2>&1; then
  say "bpf-linker not in PATH; installing with ${NIGHTLY}…"
  if ! cargo "+${NIGHTLY}" install bpf-linker --locked 2>&1; then
    say "bpf-linker install failed (common causes: missing LLVM dev headers,"
    say "insufficient disk space, MSRV mismatch with transitive deps)."
    say "Skipping ELF build. Rerun this script once bpf-linker is installed."
    exit 0
  fi
fi

say "bpf-linker: $(bpf-linker --version 2>/dev/null || echo unknown)"
say "Building lb-xdp-ebpf for bpfel-unknown-none…"
pushd "${EBPF_DIR}" >/dev/null
if cargo "+${NIGHTLY}" build --release \
      --target bpfel-unknown-none \
      -Z build-std=core 2>&1; then
  BUILT="target/bpfel-unknown-none/release/lb_xdp"
  if [ -f "${BUILT}" ]; then
    install -m 644 "${BUILT}" "${OUT_BIN}"
    say "Installed BPF ELF → ${OUT_BIN} ($(wc -c < "${OUT_BIN}") bytes)"
  else
    say "Build reported success but ${BUILT} missing; skipping install."
    popd >/dev/null
    exit 1
  fi
else
  say "cargo build failed for BPF target; leaving ebpf source as-is."
  popd >/dev/null
  exit 1
fi
popd >/dev/null
