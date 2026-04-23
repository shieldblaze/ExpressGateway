# ADR-0004: eBPF/XDP framework — aya (realised 2026-04-22 Pillar 4a, ELF + startup 2026-04-23 Pillar 4b-1)

- Status: Accepted. Pillar 4a (2026-04-22) committed aya-ebpf source +
  userspace loader. Pillar 4b-1 (2026-04-23) produced a real BPF ELF,
  embedded it into the binary, and wired optional startup attach with a
  `CAP_BPF` probe.
- Date: 2026-04-22 (Pillar 4a), 2026-04-23 (Pillar 4b-1)
- Deciders: ExpressGateway team
- Consulted: aya-rs documentation, libbpf-rs README, Katran (Meta)
  architecture, Cilium eBPF loader, "XDP paper" (Høiland-Jørgensen et al.
  2018).

## Context and problem statement

ExpressGateway advertises a "Katran-class" L4 data plane: XDP/eBPF-based
TCP and UDP packet steering with a Maglev consistent hash and a conntrack
table held in BPF maps. This ADR records how the framework question is
resolved in the repository as of Pillar 4a.

Two realistic Rust-native paths exist:

- **aya** — pure-Rust, writes both the eBPF program and the userspace
  loader in Rust (`aya-ebpf` + `aya`), uses the kernel's `BPF_PROG_LOAD`
  syscall directly, no libbpf/libelf dependency.
- **libbpf-rs** — Rust bindings over the C `libbpf` library. Battle-tested;
  brings a C toolchain into the build.

A second problem is CI: the halting gate runs `cargo test` under an
unprivileged user on a runner with no suitable NIC, no `CAP_BPF`, no
`CAP_SYS_ADMIN`, and no guaranteed BTF-enabled kernel. The tests in
`manifest/required-tests.txt` must pass in that environment.

## Decision drivers

- Rust-native toolchain alignment with the rest of the workspace.
- No C toolchain in the default workspace build — keeps `cargo build
  --workspace` reproducible.
- CI cannot load XDP programs.
- Licence: aya is MIT/Apache, compatible with our GPL-3.0-only dynamic
  linking.
- Hot-reload story: Maglev + conntrack hot-swap must be testable today,
  independently of the kernel loader.

## Considered options

1. **aya** for the eBPF program and the userspace loader.
2. **libbpf-rs** and a C eBPF program.
3. **Hybrid**: C eBPF program loaded via aya or libbpf-rs.

## Decision outcome

**Option 1: aya**, with a standalone out-of-workspace ebpf crate and an
aya userspace loader committed today. Userspace simulation remains as the
CI-safe correctness substrate.

Concretely, as of 2026-04-22:

1. `crates/lb-l4-xdp/ebpf/` is a standalone Rust crate (NOT a workspace
   member) that compiles to a BPF ELF via
   `cargo +nightly build --target bpfel-unknown-none`. Its `src/main.rs`
   is a real `#[no_std] #[no_main]` aya-ebpf XDP program — not a stub.
   The program parses Ethernet → IPv4 → TCP/UDP with per-offset bounds
   checks, consults BPF maps (`CONNTRACK`, `L7_PORTS`, `ACL_DENY`,
   `STATS`), and returns `XDP_PASS` or `XDP_DROP`.
2. `crates/lb-l4-xdp/src/loader.rs` is the userspace counterpart — an
   aya-based `XdpLoader` with `load_from_bytes`, `kernel_load`, `attach`,
   and `take_map`. Every fallible path returns `Result`; no
   `unwrap/expect/panic`.
3. `crates/lb-l4-xdp/src/lib.rs` continues to provide the userspace
   simulation (`ConntrackTable`, `MaglevTable`, `HotSwapManager`,
   `FlowKey`). The simulation is the functional spec the in-kernel
   program must satisfy; tests run on every CI push.
4. `scripts/build-xdp.sh` is a best-effort helper that compiles the ebpf
   crate to ELF and installs it next to the loader, at
   `crates/lb-l4-xdp/src/lb_xdp.bin`. `crates/lb-l4-xdp/build.rs` then
   sets `cfg(lb_xdp_elf)` so `loader::LB_XDP_ELF` is available for
   integration tests and the real launcher. If the toolchain
   (`bpf-linker`, LLVM-18 dev headers, nightly + `rust-src`) is
   unavailable the script logs and exits 0 — the ELF is optional, the
   ebpf source is authoritative.
5. The ebpf crate is **deliberately not a workspace member**. Adding it
   would break `cargo build --workspace` on any host without bpf-linker.

## Rationale

- aya's Rust-native toolchain avoids the C build step that libbpf-rs
  requires. `docker/Dockerfile` stays simple.
- Keeping the ebpf crate out of the workspace means the `bpfel-unknown-none`
  target, nightly rustc, and bpf-linker become a separate, opt-in
  build — the workspace build runs anywhere, including CI runners without
  BPF tooling.
- The simulation is not throwaway: its invariants (FIFO bounded
  conntrack, prime-sized Maglev, hot-swap stale-entry eviction) are what
  the real XDP program must match. Every simulation test is a functional
  requirement for the in-kernel code.
- aya's `Ebpf::load` returns parse/relocate errors as `EbpfError` — our
  `XdpLoader` wraps these via `#[from]`, so `-D clippy::unwrap_used`
  still passes and diagnostics remain Rust-idiomatic.
- ACL map: aya-ebpf 0.1 exposes `LpmTrie`, but its ergonomics under the
  verifier on some older kernels are fragile. Pillar 4a uses a plain
  `HashMap<u32, u32>` of /32 denies (matches what the userspace ACL in
  `lb-security` already pushes down). Pillar 4b upgrades to a real LPM
  trie with CIDR support (see follow-ups).

## Consequences

### Positive

- Real aya-ebpf source in-tree, compilable with a proper toolchain —
  not a `fn main(){}` stub.
- The workspace build and CI stay clean on any Linux or non-Linux host
  (aya is gated on `cfg(target_os = "linux")`).
- The userspace loader is testable on every CI run: garbage-ELF rejection
  returns a typed error, `XdpMode` → `XdpFlags` mapping is pinned.

### Negative

- Full XDP_TX packet rewriting (with RFC 1624 incremental checksum
  correction, VLAN stack handling, IPv6 parsing) is deferred to
  Pillar 4b. Pillar 4a's program returns `XDP_PASS` for conntrack hits
  rather than performing the kernel-side rewrite.
- `CAP_BPF`-gated integration tests (real load + attach on a test
  interface) are deferred to Pillar 4b — the CI runner cannot grant it.
- ACL is /32-only until Pillar 4b promotes the map to LPM_TRIE.

### Neutral

- The simulation will not go away when the real XDP ships. It remains
  the unit-test substrate for map / data-structure invariants.

## Implementation notes

- `crates/lb-l4-xdp/Cargo.toml` — userspace crate; `aya = { workspace =
  true }` under `[target.'cfg(target_os = "linux")'.dependencies]`.
- `crates/lb-l4-xdp/build.rs` — detects `src/lb_xdp.bin` and emits
  `cargo:rustc-cfg=lb_xdp_elf`.
- `crates/lb-l4-xdp/src/lib.rs` — simulation (unchanged); declares
  `#[cfg(target_os = "linux")] pub mod loader;`.
- `crates/lb-l4-xdp/src/loader.rs` — `XdpLoader`, `XdpMode`,
  `XdpLoaderError`. Tests: `load_garbage_bytes_rejected`,
  `load_empty_bytes_rejected`, `xdp_mode_flag_mapping`, `xdp_mode_is_copy`.
- `crates/lb-l4-xdp/ebpf/Cargo.toml` — standalone, `edition = 2024`,
  `[workspace]` empty stanza so `cargo` does not attach it to the root
  workspace if someone runs cargo inside it.
- `crates/lb-l4-xdp/ebpf/rust-toolchain.toml` — nightly + `rust-src` +
  `bpfel-unknown-none` target.
- `crates/lb-l4-xdp/ebpf/src/main.rs` — real aya-ebpf XDP program.
- `scripts/build-xdp.sh` — best-effort BPF build; documented-when-skipped.
- `Cargo.toml` (workspace) — `aya = "0.13"` added to
  `[workspace.dependencies]`.

## Follow-ups / open questions (Pillar 4b)

- Full `XDP_TX` path with RFC 1624 incremental checksum rewrite on
  conntrack hit — currently `XDP_PASS`.
- VLAN stacking (802.1Q) and IPv6 parsing.
- Promote `ACL_DENY` (HashMap<u32, u32>) to `BPF_MAP_TYPE_LPM_TRIE`
  with CIDR support. Pillar 4a defers because aya-ebpf 0.1 `LpmTrie`
  ergonomics need more exercise.
- Multi-kernel verifier matrix via an `xtask` crate — spin through
  kernel 5.15, 5.17, 6.1, 6.6 with prebuilt VMs; gate in CI behind a
  GitHub-hosted self-hosted runner with KVM.
- Real integration test: `load + attach` on a veth pair, requires
  `CAP_BPF` + `CAP_NET_ADMIN`. Gated on the CI runner capability.
- Wire `XdpLoader` into `crates/lb/src/main.rs` startup (optional,
  feature-flagged) so the LB attaches the BPF program when run with
  sufficient capabilities.

## Pillar 4b-1 realised (2026-04-23)

Between 4a and 4b-1 the LLVM / nightly situation in CI changed: the
nightly already installed on the builder was `nightly-2026-04-10`
(rustc 1.96.0-nightly), which accepts `bpf-linker 0.10.3` without any
MSRV collision. `bpf-linker` itself links against the system
`libLLVM.so.18.1`.

Concrete changes landed in Pillar 4b-1:

- `scripts/build-xdp.sh` now parses the nightly channel from
  `crates/lb-l4-xdp/ebpf/rust-toolchain.toml`, ensures `rust-src` +
  `bpfel-unknown-none` are installed for that channel, installs
  `bpf-linker` if missing, and then runs
  `cargo +<pinned-nightly> build --release --target bpfel-unknown-none
  -Z build-std=core`. On success the ELF is copied to
  `crates/lb-l4-xdp/src/lb_xdp.bin` (3 kB for the current source).
- `crates/lb-l4-xdp/src/lib.rs` exports the ELF as
  `pub const LB_XDP_ELF: &[u8]`. The bytes are emitted via an
  `#[repr(C, align(8))] Aligned<[u8; N]>` wrapper so the
  `object` crate's alignment-checked `from_bytes` cast accepts them.
- `crates/lb-l4-xdp/src/loader.rs` gained `XdpLoader::program_names`, a
  kernel-free path built on `aya_obj::Object::parse`. This lets the CI
  integration test inspect the ELF without `CAP_BPF`.
- `crates/lb-l4-xdp/tests/real_elf.rs` is a new integration test gated
  on `cfg(lb_xdp_elf)`; it runs whenever the ELF is present and asserts
  the `lb_xdp` entry is the only XDP program.
- `crates/lb-config` gained `[runtime]` with `xdp_enabled` +
  `xdp_interface`. Validation requires `xdp_interface` when
  `xdp_enabled=true`.
- `crates/lb/src/xdp.rs` owns the startup attach logic. It probes
  `CAP_BPF` then `CAP_NET_ADMIN` via the `caps` crate, and falls back
  to a `tracing::warn!` + `None` when either is missing, when the ELF
  was not compiled into the binary, or when aya reports an attach
  error. Never panics. The `XdpLoader` guard is held in
  `crates/lb/src/main.rs::async_main` so the kernel attach lasts until
  the process exits.

Observed on the CI builder (unprivileged), the binary started under a
test config with `xdp_enabled=true, xdp_interface="lo"`, logged
`xdp disabled — run the binary with CAP_BPF or as root to enable`,
and continued binding its TCP listener normally.

Remaining deferred work is now strictly Pillar 4b-2 (XDP_TX rewrite
with RFC 1624 incremental checksum, VLAN + IPv6 parsing, LPM_TRIE ACL
upgrade, multi-kernel verifier matrix via `xtask xdp-verify`, and a
`CAP_BPF`-gated real-attach CI stage on a veth pair).

## Sources

- <https://aya-rs.dev/>
- <https://github.com/libbpf/libbpf-rs>
- Høiland-Jørgensen, T. et al. "The eXpress Data Path: Fast
  Programmable Packet Processing in the Operating System Kernel" (2018).
- Meta/Katran <https://github.com/facebookincubator/katran>.
- Internal: `crates/lb-l4-xdp/src/lib.rs`,
  `crates/lb-l4-xdp/src/loader.rs`,
  `crates/lb-l4-xdp/ebpf/src/main.rs`,
  `crates/lb-l4-xdp/tests/real_elf.rs`,
  `crates/lb/src/xdp.rs`, `scripts/build-xdp.sh`.
