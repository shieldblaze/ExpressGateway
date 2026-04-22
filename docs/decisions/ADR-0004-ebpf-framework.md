# ADR-0004: eBPF/XDP framework — aya (future), userspace simulation (today)

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: aya-rs documentation, libbpf-rs README, Katran (Meta)
  architecture, Cilium eBPF loader, "XDP paper" (Høiland-Jørgensen et al.
  2018).

## Context and problem statement

ExpressGateway advertises a "Katran-class" L4 data plane: XDP/eBPF-based
TCP and UDP packet steering with a Maglev consistent hash and a conntrack
table held in BPF maps. There are two realistic Rust-native paths to
produce and load the eBPF program:

- **aya** — pure-Rust, writes both the eBPF program and the userspace
  loader in Rust (`aya-bpf` + `aya`), uses the kernel's
  `BPF_PROG_LOAD` syscall directly, no libbpf/libelf dependency.
- **libbpf-rs** — Rust bindings over the C `libbpf` library, which owns
  ELF handling, CO-RE relocations, and BTF. Battle-tested, used by Cilium
  and BCC; brings a C toolchain into the build.

Separately, the halting gate's CI must pass on a runner that *cannot* load
XDP programs (no `CAP_SYS_ADMIN`, no suitable NIC, and no kernel BTF).
That's a hard constraint: the tests in `manifest/required-tests.txt` run
under `cargo test` with no special privileges.

The honest situation today is that `lb-l4-xdp` is a **userspace
simulation** of the data-plane semantics. The crate-level doc comment
states this explicitly (`crates/lb-l4-xdp/src/lib.rs:1–5`):

    //! L4 XDP/eBPF data plane for TCP and UDP load balancing.
    //!
    //! Provides userspace simulation of the L4 XDP data plane. Real eBPF programs
    //! cannot be tested in CI, so we simulate the conntrack table, Maglev consistent
    //! hashing, and hot-swap behavior.

A stub eBPF entry point exists in `crates/lb-l4-xdp/ebpf/src/main.rs` and
its comment references aya:

    //! This is a stub for the eBPF program that will be compiled separately
    //! using the aya-bpf toolchain. It is not compiled as part of the normal
    //! workspace build.

So this ADR records two decisions: (a) when we do build a real eBPF
program, we use **aya**, and (b) today we keep the simulation and gate
the real program behind a separate build.

## Decision drivers

- Rust-native toolchain alignment with the rest of the workspace
  (edition 2024, `#![deny(clippy::unwrap_used, …)]`).
- No C toolchain in the default build — keeps the `cargo build` story
  reproducible on any supported Rust target.
- CI cannot load XDP (no privileges, no BTF-enabled kernel guaranteed).
- Licence: aya and libbpf-rs both MIT/Apache; workspace is GPL-3.0-only,
  both compatible with dynamic linking against the kernel's BPF
  subsystem.
- Hot-reload story: the Maglev + conntrack hot-swap logic is the same
  whether the data plane is a BPF program or a Rust simulation. We want
  to develop and test that logic *now*, separately from the kernel
  loader.
- Testability: 1 734+ tests in the workspace (per MEMORY.md); the L4
  suite must contribute without flakiness.
- Debuggability: `aya` surfaces verifier errors as Rust errors; libbpf's
  log output is C-idiomatic.

## Considered options

1. **aya** for both the eBPF program and the userspace loader.
2. **libbpf-rs** and a C (or rust-bpf) eBPF program.
3. **Hybrid**: write the eBPF program in C and load it via aya or
   libbpf-rs.
4. **Userspace simulation only** in `lb-l4-xdp`, with the real eBPF
   build as a deferred, out-of-workspace concern.

## Decision outcome

Adopt options **1 + 4**: aya is the chosen framework for when we build
the real data plane, and until then `lb-l4-xdp` is a userspace simulation
that the rest of the workspace depends on via typed APIs
(`ConntrackTable`, `MaglevTable`, `HotSwapManager`, `FlowKey`).

The eBPF crate (`crates/lb-l4-xdp/ebpf/src/main.rs`) is a stub with a
`fn main() {}` body and is explicitly marked "not compiled as part of the
normal workspace build."

## Rationale

- Pure-Rust toolchain: aya fits the workspace philosophy. libbpf-rs
  would introduce a C build step that complicates the `docker/Dockerfile`
  and the release pipeline.
- Verifier feedback: aya's verifier integration surfaces Rust
  diagnostics; this preserves our developer ergonomics story.
- CO-RE: aya supports CO-RE via `aya-gen`, adequate for our conntrack
  map + Maglev table, which are both simple BTF-representable types.
- Simulation-first preserves the test surface: `ConntrackTable` with
  FIFO eviction, `MaglevTable` with a prime table size, and
  `HotSwapManager` that preserves in-flight flows after a backend swap
  are all exercised by deterministic unit tests in
  `crates/lb-l4-xdp/src/lib.rs` (tests module lines 377+).
- The simulation's invariants are the same invariants the real eBPF
  program must satisfy — so the tests act as a functional spec for the
  eventual kernel code.
- Cargo.lock contains no `aya`, `aya-bpf`, or `libbpf-rs` today. This
  ADR records that when we do add it, aya wins.

## Consequences

### Positive
- The L4 semantics (conntrack eviction, Maglev permutation ordering,
  hot-swap safety) are testable on every CI run without root.
- Picking aya now locks the Cargo.toml dep surface to one framework;
  avoids the "is it libbpf this week?" question.
- Consistent panic-free lint story: the simulation's `lib.rs` runs under
  the project `#![deny(…)]` block; when the eBPF crate is added, it
  inherits the same discipline.

### Negative
- No real packet processing in CI — we rely on simulation fidelity.
- The documented "Katran-class" performance is aspirational until the
  real XDP program is wired up.
- aya's ecosystem is smaller than libbpf's; if we hit a toolchain
  limitation (e.g. a BTF feature that aya lags on), we may have to
  temporarily fall back.

### Neutral
- The simulation will not go away when real XDP ships: it remains the
  unit-test substrate for the map/data-structure invariants.

## Implementation notes

- `crates/lb-l4-xdp/src/lib.rs` — simulation: `FlowKey`, `ConntrackTable`,
  `MaglevTable`, `HotSwapManager`.
- `crates/lb-l4-xdp/ebpf/src/main.rs` — stub eBPF entry point, comment
  points at aya-bpf.
- `crates/lb-l4-xdp/Cargo.toml` — userspace deps only (`thiserror`,
  `rand`, `parking_lot`).
- Tests: `tests/l4_xdp_conntrack.rs` plus the module-level tests.

## Follow-ups / open questions

- When do we build the real aya-based XDP program? Gated on a CI runner
  with kernel ≥ 6.1 BTF and root/setcap.
- Map-type choice for conntrack in the kernel: LRU_HASH vs HASH with
  user-space eviction. ADR-0005 records the simulation choice (FIFO);
  LRU_HASH is almost certainly what we pick in-kernel.
- Maglev table size tuning — current tests use 65 537 (prime); production
  may want a larger prime.

## Sources

- <https://aya-rs.dev/>
- <https://github.com/libbpf/libbpf-rs>
- Høiland-Jørgensen, T. et al. "The eXpress Data Path: Fast
  Programmable Packet Processing in the Operating System Kernel" (2018).
- Meta/Katran <https://github.com/facebookincubator/katran>.
- Internal: `crates/lb-l4-xdp/src/lib.rs`, `crates/lb-l4-xdp/ebpf/src/main.rs`.
