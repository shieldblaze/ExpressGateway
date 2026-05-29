#!/usr/bin/env bash
# S15 A2-7 — NEVER-DECRYPTED LINKAGE proof.
#
# audit/quic/s15-design.md owner ruling §9.5 primary item 1
# (verbatim):
#
#   "PRIMARY (load-bearing, by-construction):
#    1. Linkage: `cargo bloat -p lb-quic --filter quiche` shows
#       ZERO quiche::Connection / BoringSSL decrypt symbols on the
#       Mode A passthrough path, with cfg(not(feature =
#       "quic-passthrough-only")) guards around any
#       quiche::Connection / crypto use."
#
# Target is the **lb-quic CRATE** (lib), not the lb BINARY. The
# binary side has its own (still-open) gating gap tracked under
# CF-S15-LB-BIN-FEATURE-GATING — see commit body. The owner's
# proof binds at the lb-quic boundary: `quic-terminate` off,
# `quic-passthrough-only` on, no quiche::Connection symbol in the
# crate's compiled artifact.
#
# Method:
#   1. Build `lb-quic` (release) with the gating combo above.
#   2. `cargo bloat -p lb-quic` (symbol-level, --filter quiche) →
#      /tmp/never-decrypted-bloat.txt.
#   3. grep for termination/decryption symbols (quiche::Connection,
#      BoringSSL handshake entry points). Hit ⇒ FAIL.
#
# Expected: PASS — lb-quic's quic-terminate-gated mod tree
# (router.rs, conn_actor.rs, h3_bridge.rs, listener.rs, quiche
# dep, tokio-quiche dep, lb-h3 dep) is excluded under
# `--no-default-features --features quic-passthrough-only`, so
# nothing in the lb-quic compilation unit references quiche.
#
# Exit codes:
#   0 — PASS: zero terminating-side symbols on the lb-quic Mode A
#       compilation unit.
#   1 — FAIL: quiche / BoringSSL symbol found (offending lines
#       echoed; see /tmp/never-decrypted-bloat.txt for full output).
#       Indicates the cfg-gate around quiche-bearing modules has
#       regressed.
#   2 — TOOLING: `cargo bloat` missing; advise install. Not a
#       proof failure but a verification blocker.
#
# CARRY-FORWARD: **CF-S15-LB-BIN-FEATURE-GATING** — the `lb`
# binary itself still links quiche via THREE paths under
# `--no-default-features --features quic-passthrough-only`:
#   1. `lb` → `quiche` direct dep (lb/Cargo.toml; main.rs:52 H3
#      upstream-pool `quiche::Config` factory).
#   2. `lb` → `lb-l7` → `lb-quic` (default features = quic-terminate).
#   3. `lb` → `lb-quic` (gated default-features=false by S15 A2-8) —
#      this path IS clean.
# Closing the binary-side gap is a Phase A3 candidate (or
# post-S15 carry-forward) that needs:
#   - `lb-l7` features mirror (default = ["quic-terminate"],
#     `quic-passthrough-only` marker) + cfg-gating on H3-upstream
#     paths inside lb-l7.
#   - `lb/Cargo.toml`: gate `quiche` direct dep + `lb-l7` dep
#     behind `quic-terminate`.
#   - `lb/src/main.rs`: cfg-gate spawn_quic + every
#     QuicListener / Http2Pool / H3 upstream factory call-site.
# Symbol footprint to close (today): quiche::Connection 100+ KiB,
# bssl::ssl_*_handshake 25 KiB.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# CARGO_TARGET_DIR is shared with the rest of the workspace
# tooling (see CLAUDE.md / spawn brief). Falls back to ./target
# when unset.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/home/ubuntu/Code/eg-target}"

BLOAT_OUT="${BLOAT_OUT:-/tmp/never-decrypted-bloat.txt}"

# Step 0 — tooling probe. `cargo bloat` is a third-party
# subcommand; we don't auto-install (slow, and the script is
# expected to run inside CI/verifier sessions that pre-warm the
# toolchain).
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
