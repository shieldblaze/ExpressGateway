#!/usr/bin/env bash
# S15 A2-7 — NEVER-DECRYPTED LINKAGE proof (audit/quic/s15-design.md §9.5).
#
# Builds lb-quic with `--no-default-features --features quic-passthrough-only` and
# greps `cargo bloat --filter quiche` for quiche::Connection / BoringSSL handshake
# symbols. Any hit = FAIL = the cfg-gate around the quiche-bearing mod tree has
# regressed. Exit 0 PASS, 1 FAIL, 2 `cargo bloat` missing (blocker, not a failure).
#
# SCOPE: this binds at the **lb-quic CRATE** boundary, NOT the `lb` BINARY.
#
# CARRY-FORWARD **CF-S15-LB-BIN-FEATURE-GATING** (this file is its only record):
# under the same feature combo the `lb` binary STILL links quiche by two of three
# paths — (1) its direct `quiche` dep, used by the H3 upstream-pool Config factory,
# and (2) `lb` -> `lb-l7` -> `lb-quic` with lb-l7's default quic-terminate. Only
# (3) `lb` -> `lb-quic` is gated clean (S15 A2-8). Closing it needs a feature
# mirror on lb-l7 plus cfg-gating of the spawn_quic / QuicListener / H3-upstream
# call-sites. Footprint: quiche::Connection 100+ KiB, bssl::ssl_*_handshake 25 KiB.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Shared with the rest of the workspace tooling; falls back to ./target.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/home/ubuntu/Code/eg-target}"

BLOAT_OUT="${BLOAT_OUT:-/tmp/never-decrypted-bloat.txt}"

# `cargo bloat` is a third-party subcommand; deliberately NOT auto-installed —
# callers are CI/verifier sessions that pre-warm the toolchain.
if ! cargo bloat --version >/dev/null 2>&1; then
    echo "FAIL: cargo bloat not installed"
    echo "REMEDIATION: cargo install cargo-bloat --locked"
    exit 2
fi

# Step 1 — build the lb-quic Mode A linkage probe example
# (release, default features OFF + quic-passthrough-only ON).
# `cargo bloat` cannot inspect an rlib directly, so we attach to
# a tiny `examples/passthrough_linkage_probe.rs` that takes
# function pointers to the Mode A public surface
# (`PassthroughListener::spawn`, etc.) — this forces the linker
# to include lb-quic's compiled Mode A code, while leaving every
# `cfg(feature = "quic-terminate")` module out.
echo ">>> building lb-quic --example passthrough_linkage_probe (release, --no-default-features --features quic-passthrough-only)"
cargo build \
    -p lb-quic \
    --example passthrough_linkage_probe \
    --release \
    --no-default-features \
    --features quic-passthrough-only

# Step 2 — bloat the probe example at symbol granularity,
# filtered to quiche-attributed symbols. `-n 100` is enough to
# surface even minor termination-side residue; the filter
# narrows the attention surface so a small regression is obvious.
echo ">>> cargo bloat -p lb-quic --example passthrough_linkage_probe --filter quiche -n 100"
cargo bloat \
    -p lb-quic \
    --example passthrough_linkage_probe \
    --release \
    --no-default-features \
    --features quic-passthrough-only \
    --filter quiche \
    -n 100 \
    | tee "$BLOAT_OUT"

# Step 3 — assert. The grep is anchored on the termination /
# decryption surfaces that MUST be absent in Mode A:
#
#  - `quiche::Connection`: the quiche termination state machine
#    entry point. Send/recv/handshake all route through it.
#  - `boring_sys::` / `bssl::` / `BoringSSL`: the BoringSSL
#    handshake + AEAD primitives quiche links for TLS 1.3.
#  - `boring::`: the higher-level Rust wrapper crate.
#  - `ssl_server_handshake` / `ssl_client_handshake`: the
#    name-mangled BoringSSL handshake entry points.
#
# A hit on ANY of these means termination code reached the lb-quic
# Mode A compilation unit (i.e. a cfg-gate around the H3
# router/actor/bridge/listener tree has regressed).
TERMINATION_RE='(quiche::Connection|boringssl|BoringSSL|boring_sys::|boring::|bssl::|ssl_server_handshake|ssl_client_handshake)'
if grep -qE "$TERMINATION_RE" "$BLOAT_OUT"; then
    echo
    echo "FAIL: quiche / BoringSSL symbols present on the lb-quic"
    echo "      Mode A compilation unit — the cfg-gate around the"
    echo "      H3 termination tree has regressed."
    echo
    echo "Offending lines:"
    grep -nE "$TERMINATION_RE" "$BLOAT_OUT" | head -20 || true
    echo
    echo "Full output: $BLOAT_OUT"
    echo "REMEDIATION: code-read the call chain that pulls in the"
    echo "  flagged symbol; ensure the offending module is gated"
    echo "  on \`cfg(feature = \"quic-terminate\")\` per"
    echo "  CF-S15-PASSTHROUGH-FEATURE-GATING (lb-quic/src/lib.rs)."
    exit 1
fi

echo
echo "PASS: never_decrypted_proof LINKAGE — zero quiche::Connection"
echo "      / BoringSSL symbols on the lb-quic Mode A compilation"
echo "      unit. Full output: $BLOAT_OUT"
echo
echo "NOTE: CF-S15-LB-BIN-FEATURE-GATING — the lb BINARY still"
echo "      links quiche under the same feature combo via lb-l7"
echo "      and the direct quiche dep in lb/Cargo.toml. That gap"
echo "      is tracked separately and is NOT covered by this gate."
