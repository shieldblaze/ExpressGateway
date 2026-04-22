#!/usr/bin/env bash
# Pillar 4a: best-effort build of the standalone BPF crate.
#
# This is intentionally out-of-workspace: requires bpf-linker + LLVM-18 dev
# headers + nightly rustc with rust-src, none of which participate in
# `cargo build --workspace`. If the toolchain is unavailable the script
# exits 0 after logging the reason — the ebpf source is authoritative, the
# ELF is opportunistic.
#
# On success the ELF is copied to crates/lb-l4-xdp/src/lb_xdp.bin and the
# userspace crate's build.rs picks it up via the `lb_xdp_elf` cfg so the
# `LB_XDP_ELF` constant becomes available.
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$(pwd)"
EBPF_DIR="${REPO_ROOT}/crates/lb-l4-xdp/ebpf"
OUT_BIN="${REPO_ROOT}/crates/lb-l4-xdp/src/lb_xdp.bin"

say() { printf 'build-xdp.sh: %s\n' "$*"; }

if ! command -v bpf-linker >/dev/null 2>&1; then
  say "bpf-linker not in PATH; attempting 'cargo install bpf-linker --locked'"
  if ! cargo install bpf-linker --locked 2>&1; then
    say "bpf-linker install failed (often: missing LLVM-18 dev headers)."
    say "Skipping BPF ELF build. ebpf source is authoritative; rerun this"
    say "script once LLVM-18 dev + bpf-linker are installed."
    exit 0
  fi
fi

if ! rustup toolchain list 2>/dev/null | grep -q '^nightly'; then
  say "nightly rustc not installed; cannot target bpfel-unknown-none. Skipping."
  exit 0
fi

say "Building lb-xdp-ebpf for bpfel-unknown-none…"
pushd "${EBPF_DIR}" >/dev/null
if cargo +nightly build --release \
      --target bpfel-unknown-none \
      -Z build-std=core 2>&1; then
  BUILT="target/bpfel-unknown-none/release/lb_xdp"
  if [ -f "${BUILT}" ]; then
    install -m 644 "${BUILT}" "${OUT_BIN}"
    say "Installed BPF ELF → ${OUT_BIN}"
  else
    say "Build reported success but ${BUILT} missing; skipping install."
  fi
else
  say "cargo build failed for BPF target; leaving ebpf source as-is."
  popd >/dev/null
  exit 0
fi
popd >/dev/null
