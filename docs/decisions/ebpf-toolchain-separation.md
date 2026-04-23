# ADR: eBPF toolchain lives separately from the workspace

- Status: Accepted
- Date: 2026-04-23
- Deciders: ExpressGateway team
- Related: ADR-0004 (eBPF framework), ADR-0005 (BPF map schema), Pillar 4a commit `35491253`, Pillar 4b acceptance criteria

## Context

`crates/lb-l4-xdp/ebpf/src/main.rs` is a real `aya-ebpf` XDP program (Pillar 4a). Building it into a loadable BPF ELF requires the `bpf-linker` binary plus LLVM-21 development headers. The workspace MSRV is **1.85 stable** (`rust-toolchain.toml` at the repo root).

During Pillar 4a, attempting to `cargo install bpf-linker --locked` under the workspace toolchain failed: `bpf-linker`'s own transitive dependency graph currently requires rustc ≥ 1.88 (`cargo-platform 0.3.2`, `libloading 0.9.0`, `time 0.3.47`). So the BPF ELF could not be produced in that drive.

The naïve fix — bump the workspace MSRV to 1.88 — was rejected because:

1. Pillar 3b's new QUIC stack (quiche + tokio-quiche; see companion ADR `quinn-to-quiche-migration.md`) has MSRV 1.85. Bumping to 1.88 would be required only by bpf-linker.
2. The workspace's 40+ crates and their transitive graph are verified on 1.85 today. An MSRV bump carries compile-time + dependency-resolution risk that isn't offset by benefit.
3. The BPF program is a **separate compilation artifact**, not a crate the workspace links against. It does not need to share a toolchain with the userspace workspace.

This ADR codifies the two-toolchains-one-repo pattern used by other production aya-based projects (`katran`, Cloudflare's eBPF tools, Aya's own examples).

## Decision

1. **Workspace (all `crates/lb-*` except `crates/lb-l4-xdp/ebpf/`)** stays on stable 1.85, pinned by the root `rust-toolchain.toml`.
2. **The ebpf sub-crate** (`crates/lb-l4-xdp/ebpf/`) keeps its own `rust-toolchain.toml` pinning a nightly with LLVM-21-target support, plus the `rust-src` component and the `bpfel-unknown-none` target. The nightly version is pinned to a date stamp that bpf-linker's current release accepts; roll forward when bpf-linker raises its MSRV.
3. **`bpf-linker` is installed with the nightly toolchain explicitly**:
   ```
   cargo +nightly-<pinned-date> install bpf-linker --locked
   ```
   The binary lands in `~/.cargo/bin/bpf-linker` and is toolchain-independent at runtime.
4. **`scripts/build-xdp.sh`** drives the BPF ELF build using `cargo +nightly-<pinned-date> build --release --target bpfel-unknown-none -Z build-std=core` inside `crates/lb-l4-xdp/ebpf/`. It emits `crates/lb-l4-xdp/src/lb_xdp.bin`. The build is best-effort in this drive: if bpf-linker or LLVM-21 isn't present, the script logs a remediation message and exits 0. When Pillar 4b lands, the script becomes a hard dependency of the integration test that actually loads the ELF under CAP_BPF.
5. **Workspace `cargo build` does NOT descend into `crates/lb-l4-xdp/ebpf/`.** The ebpf crate is intentionally absent from the `[workspace] members` list in the root `Cargo.toml` (see Pillar 4a). This keeps `cargo build --workspace --release` green on stable 1.85 without bpf-linker on the PATH.

## Consequences

### Positive

- No workspace MSRV cascade. The 1.85→1.88 bump is cancelled, alongside its ripple effects on the rest of the dependency graph.
- Each side of the boundary moves at its own pace. The ebpf nightly can roll forward whenever bpf-linker or aya-ebpf raises its pin; the userspace workspace stays on long-supported stable.
- Production users who deploy the binary but don't run XDP don't need a BPF toolchain at all — the workspace build succeeds without it.
- Integration-test authors know exactly where the boundary is: an XDP test invokes `scripts/build-xdp.sh` and then loads the ELF; it doesn't require anything else.

### Negative

- **Two toolchains to maintain.** Contributors touching the BPF program install the pinned nightly + bpf-linker + LLVM-21. Documented in `DEPLOYMENT.md` and CONTRIBUTING (when it exists).
- **`scripts/build-xdp.sh` is a parallel build system.** It is a short bash script, not a `cargo` subcommand; diligence required to keep in sync with the workspace's own build profile (panic, LTO, codegen-units).
- **CI complexity.** A full CI matrix needs two toolchain installs when it builds both the workspace and the BPF ELF. For the halting-gate this isn't needed (the gate only runs the workspace tests); for Pillar 4b's `xtask xdp-verify` multi-kernel matrix it is.

### Neutral

- The pattern matches what Cloudflare's katran, Cilium's aya-based components, and the Aya project's own examples do. Zero novelty; easy for new contributors to recognize.

## How this differs from ADR-0004

ADR-0004 selected aya as the userspace loader framework and introduced the simulation-first posture. This ADR formalizes the build-system boundary between the userspace workspace (which aya is a dep of) and the eBPF program (which aya-ebpf is a dep of). ADR-0004 continues to cover the aya vs. libbpf-rs choice and the simulation-for-CI strategy; this ADR covers where each crate compiles.

## Implementation notes

- `crates/lb-l4-xdp/ebpf/rust-toolchain.toml` already exists from Pillar 4a. On the next bpf-linker install attempt, bump the `channel = "nightly-<YYYY-MM-DD>"` date to whatever the current bpf-linker `cargo install --locked` accepts. That's the only maintenance pin.
- `scripts/build-xdp.sh` already exists from Pillar 4a; extend it to pass `cargo +$(cat crates/lb-l4-xdp/ebpf/rust-toolchain.toml | grep channel | ...)` explicitly if needed.
- Document in `DEPLOYMENT.md` that the BPF ELF build is optional for pure-L7 deployments and required only when XDP acceleration is enabled.

## Follow-ups

- When aya publishes 0.13+ with stable-toolchain compat for the ebpf side, collapse the two toolchains into one. (Not currently on any public aya roadmap.)
- Add a `cargo xtask bpf-build` wrapper so contributors don't need to know the exact `cargo +nightly-... -Z build-std=core` invocation.

## Sources

- aya book: <https://aya-rs.dev/book/>
- bpf-linker: <https://github.com/aya-rs/bpf-linker>
- Cargo `rust-toolchain.toml` precedence rules: <https://rust-lang.github.io/rustup/overrides.html>
- Pillar 4a commit: `35491253` (ExpressGateway `main`)
- ADR-0004 — eBPF framework
- ADR-0005 — BPF map schema
