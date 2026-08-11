#!/usr/bin/env bash
# Build the standalone eBPF crate into a loadable BPF ELF at
# crates/lb-l4-xdp/src/lb_xdp.bin, which build.rs detects to emit cfg(lb_xdp_elf)
# and expose lb_l4_xdp::LB_XDP_ELF.
#
# The ebpf crate needs its OWN nightly (crates/lb-l4-xdp/ebpf/rust-toolchain.toml),
# not the workspace stable — aya-ebpf + bpf-linker require it.
#
# Exits 0 even when bpf-linker is absent, logging the remediation instead: the
# committed ELF stays in place so downstream builds keep working.

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

# Do NOT add `--target bpfel-unknown-none` here: that target has no prebuilt
# rust-std (we build `core` from source via -Z build-std), so rustup tries to
# download a nonexistent component and fails — which `|| exit 0` then turned into a
# silent "skip ELF build" leaving a STALE committed lb_xdp.bin. Round-8 D-1/D-2.
if ! rustup toolchain list 2>/dev/null | grep -q "^${NIGHTLY}\|^nightly"; then
  say "Installing ${NIGHTLY} toolchain…"
  rustup toolchain install "${NIGHTLY}" --component rust-src --profile minimal || {
    say "rustup toolchain install ${NIGHTLY} failed. Skipping ELF build."
    exit 0
  }
else
  # build-std needs rust-src on the pinned channel.
  rustup component add rust-src --toolchain "${NIGHTLY}" >/dev/null 2>&1 || true
fi

# bpf-linker must match rustc's LLVM major — install it with the pinned nightly.
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
# EBPF-2-01: `-Cdebuginfo=2` emits the DWARF that `--btf` lowers to BTF/BTF.ext.
# Do NOT re-add `-C link-arg=-g` — bpf-linker 0.10.3 errors `unexpected argument
# '-g'`; debuginfo=2 + --btf alone are sufficient.
export RUSTFLAGS="${RUSTFLAGS:-} -Cdebuginfo=2 -Clink-arg=--btf"
pushd "${EBPF_DIR}" >/dev/null
if cargo "+${NIGHTLY}" build --release \
      --target bpfel-unknown-none \
      -Z build-std=core 2>&1; then
  BUILT="target/bpfel-unknown-none/release/lb_xdp"
  if [ -f "${BUILT}" ]; then
    # Strip .debug_* only AFTER BTF generation: debuginfo=2 leaves ~90 KiB the
    # kernel never reads, and the ELF must land under build.rs's MAX_ELF_BYTES
    # (64 KiB) or it panics instead of embedding. ~180 KiB -> ~36 KiB.
    STRIP="$(command -v llvm-objcopy-21 || command -v llvm-objcopy || command -v objcopy)"
    if [ -n "${STRIP}" ]; then
      "${STRIP}" --strip-debug "${BUILT}" "${BUILT}.lean" && BUILT="${BUILT}.lean"
      say "stripped .debug_* via ${STRIP##*/}"
    else
      say "WARN: no objcopy found; installing un-stripped ELF (may exceed MAX_ELF_BYTES)"
    fi
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
