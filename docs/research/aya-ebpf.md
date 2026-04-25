# aya: Rust eBPF framework

## What aya provides

aya is a pure-Rust eBPF framework with two halves: a *userspace* crate
(`aya`) that loads, attaches, and interacts with eBPF programs, and a
*BPF-side* crate (`aya-ebpf`) that you link into a `no_std`, no-alloc
program compiled to the BPF target. Unlike `libbpf-rs`, aya does not
depend on libbpf; the loader, ELF relocation handler, and CO-RE
(Compile Once, Run Everywhere) relocation logic are reimplemented in
Rust.

On the userspace side, aya exposes:

- `Ebpf::load()` — parses a BPF object file, validates sections,
  applies BTF-based CO-RE relocations, and prepares programs and maps
  for loading.
- `Program` abstractions per BPF program type: `Xdp`, `TracePoint`,
  `Uprobe`, `SocketFilter`, etc. Each has an `attach` method matching
  its kernel hook.
- Typed map wrappers: `HashMap<K, V>`, `LruHashMap<K, V>`,
  `PerCpuArray<T>`, `RingBuf`, `ProgramArray`, `SockMap`, etc.,
  providing safe read/update/delete from userspace.
- `aya-log` — a structured logger that bridges `bpf_printk`-style
  messages from a running BPF program to the userspace `tracing`
  ecosystem.

On the BPF side, `aya-ebpf` gives us:

- `#[xdp]`, `#[tracepoint]`, `#[kprobe]`, `#[socket_filter]` attribute
  macros that emit the correct ELF section name.
- `XdpContext`, `TcContext`, etc., wrappers around the raw kernel
  context pointer with safe helpers such as `data()`, `data_end()`,
  `ptr_at(offset)` that perform the bounds checks the verifier
  requires.
- Map declarations via `#[map] static FOO: HashMap<K, V> = ...` which
  expand to ELF sections the loader picks up.

The codegen story is: write Rust that targets `bpfel-unknown-none`
(little-endian BPF, no OS), compile with `cargo xtask`-style
tooling, and emit an ELF the userspace loader consumes. `aya-build`
integrates this into Cargo so a single `cargo build` can produce both
halves of the system.

## Why aya and not libbpf-rs or bcc

Three concrete reasons plus one organisational one.

1. **Rust-native, no C toolchain.** `libbpf-rs` binds to libbpf, which
   is a C library that itself depends on libelf and zlib. Our CI must
   build the BPF object on any contributor's laptop; a
   `cargo install && cargo build` flow is vastly cheaper than
   apt-installing `libbpf-dev`, `libelf-dev`, and matching the
   host kernel's header tree. aya sidesteps this entirely.
2. **Unified error model.** Errors from the BPF side (verifier
   rejections, helper mismatches) and from the userspace side (map
   open failures, attach failures) are `thiserror`-derived Rust types
   the whole way through. We can `?` them into our existing
   `XdpError` type.
3. **`aya-log` integration.** Debugging a BPF program is notoriously
   painful. `aya-log` lets us emit structured logs from kernel space
   that appear in the same `tracing` subscriber as the userspace
   logs, with the same level filtering.
4. **Organisational:** aya has momentum (adopted by Cilium's Tetragon,
   the eunomia project, Red Hat's bpfman), an active maintainer
   group, and aligns with our all-Rust posture.

bcc (BPF Compiler Collection) is Python-first and recompiles the BPF
program at agent startup via clang; that is unacceptable for a
production data plane. A future ADR
(`docs/decisions/ADR-0004-ebpf-framework.md` is reserved) will formally
record the aya choice.

## Verifier constraints for our L4 XDP program

The in-kernel BPF verifier walks every path of the program before
attach and proves termination, memory safety, and helper-function
discipline. A `Result<Xdp, ...>` that returns to userspace means
**every** reachable instruction was verified. The constraints that
shape our L4 design are:

- **Bounded loops.** The verifier requires every loop to either (a)
  have a constant trip count small enough to unroll
  (`#pragma unroll`, or `for i in 0..N` with `N` known), or (b) use
  the `bpf_loop` helper (kernel ≥ 5.17) which takes a maximum
  iteration count as an argument. Maglev lookup table construction
  cannot run inside the BPF program; we build the table in userspace
  and push it into a `BPF_MAP_TYPE_ARRAY`. The hot-path lookup is
  then `table[hash % table_size]` — a single bounded operation.
- **Bounded stack.** 512 bytes per program, hard. We cannot declare
  large local arrays; all state lives in maps.
- **No unbounded recursion.** BPF has no stack frame for recursive
  calls without `bpf_tail_call`.
- **No heap.** There is no `kmalloc` inside BPF. Dynamic storage
  means maps.
- **Map size caps.** The LRU conntrack map needs a `max_entries`
  declared at load time; kernel preallocates. 1M entries at 32 bytes
  per flow is 32 MiB, acceptable. See ADR-0005 placeholder for the
  map schema decision.
- **Helper whitelist.** Each program type has a set of permitted
  helpers. For XDP on Linux 5.15: `bpf_map_lookup_elem`,
  `bpf_map_update_elem`, `bpf_map_delete_elem`, `bpf_xdp_adjust_head`,
  `bpf_xdp_adjust_tail`, `bpf_redirect_map`, `bpf_csum_diff`,
  `bpf_l3_csum_replace`, and the `bpf_fib_lookup` helper for FIB
  resolution. `bpf_loop` on 5.17+, `bpf_kfunc` pattern on 5.18+ for
  custom kernel functions. We stay inside the 5.15 baseline to match
  the LTS target documented in PROMPT.md.
- **Pointer provenance / bounds.** Every pointer read must be
  preceded by a bounds check proving the read does not cross
  `data_end`. `XdpContext::ptr_at` in aya-ebpf does this
  automatically.

## What ExpressGateway's lb-l4-xdp actually is

`crates/lb-l4-xdp/src/lib.rs` is explicit: it is a **userspace
simulation** of the L4 XDP data plane. The module-level doc comment
reads: *"Provides userspace simulation of the L4 XDP data plane. Real
eBPF programs cannot be tested in CI, so we simulate the conntrack
table, Maglev consistent hashing, and hot-swap behavior."*

The simulation implements:

- `ConntrackTable` — flow-5-tuple → backend-index map with bounded
  capacity and FIFO eviction (`DEFAULT_CONNTRACK_MAX_ENTRIES =
  1_000_000`). Real kernel version would be a `BPF_MAP_TYPE_LRU_HASH`.
- `MaglevTable` — Maglev consistent hash with prime-sized lookup
  table (the permutation algorithm is the standard offset/skip form
  from the Maglev paper). Real kernel version would be a
  `BPF_MAP_TYPE_ARRAY` populated from userspace on each backend-set
  change.
- `HotSwapManager` — atomically swaps the Maglev table while
  preserving existing conntrack entries. Stale conntrack entries
  (pointing at a backend index no longer in range) are detected and
  re-routed on next lookup.

The sibling directory `crates/lb-l4-xdp/ebpf/src/main.rs` is a stub
entry point with an empty `main()` and the comment *"not compiled as
part of the normal workspace build."* It exists to anchor the future
BPF-side source and to be picked up by `cargo build --target
bpfel-unknown-none` once the toolchain is wired.

This structure is deliberate: the semantic correctness of conntrack
eviction, Maglev permutation stability, and hot-swap handover is
tested in `tests/l4_xdp_conntrack.rs` and the unit tests in
`lib.rs`. The performance and kernel-integration correctness will be
proved by a separate, out-of-CI benchmark once the BPF object
compiles.

## Path to real deployment

1. **Compile the BPF object.** Install a BPF-capable `rustc` target
   (`rustup target add bpfel-unknown-none`), add `aya-ebpf` and
   `aya-log-ebpf` as BPF-side dependencies, and port the
   `MaglevTable::lookup` and `HotSwapManager::route_flow` logic to
   use `#[xdp]`-annotated functions with `XdpContext`.
2. **Userspace loader.** Replace the simulation's `HotSwapManager`
   methods with calls to `aya::Ebpf::map_mut` that update a
   userspace-held pinned `ArrayMap` for the Maglev lookup table
   and a `LruHashMap` for conntrack.
3. **XDP attach modes.** XDP supports three attach modes: **SKB**
   (generic, works on any driver, slowest — runs after the kernel
   has already built an `sk_buff`), **DRV** (native driver, normal
   path, no `sk_buff` allocation), and **HW** (NIC offload,
   hardware-dependent). Our production target is DRV on ixgbe /
   mlx5 / ena; SKB is an acceptable fallback for development VMs.
   aya exposes `XdpFlags::SKB_MODE`, `DRV_MODE`, `HW_MODE`.
4. **Verification tooling.** After attach, `bpftool prog show`
   lists loaded programs; `bpftool prog dump xlated` shows the
   verifier's view of the instruction stream; `bpftool map dump`
   inspects map contents. Our integration tests will assert these
   outputs match expectations.
5. **Alternative loader.** If aya becomes a blocker, the aya-compiled
   ELF can be loaded via a libbpf skeleton (`libbpf-rs`) instead.
   The BPF object format is standardised — the loader is an
   implementation detail. We keep the BPF-side code under
   `crates/lb-l4-xdp/ebpf/` so this switch is mechanical.

The "deferred real deployment" posture is acceptable because the
XDP L4 tier is an acceleration, not a correctness requirement: the
tokio-based L4 path via `lb-io` can proxy TCP and UDP flows directly
when XDP is unavailable (PROMPT.md §2 "XDP vs io_uring Decision
Matrix").

## Sources

- aya book, https://aya-rs.dev/book/
- aya source, https://github.com/aya-rs/aya
- Linux kernel BPF documentation,
  https://www.kernel.org/doc/html/latest/bpf/index.html
- BPF verifier notes,
  https://www.kernel.org/doc/html/latest/bpf/verifier.html
- XDP paper, Høiland-Jørgensen et al., "The eXpress Data Path"
  (CoNEXT '18),
  https://dl.acm.org/doi/10.1145/3281411.3281443
- Maglev paper, Eisenbud et al., "Maglev: A Fast and Reliable
  Software Network Load Balancer" (NSDI '16),
  https://research.google/pubs/pub44824/
- Katran (Facebook), https://github.com/facebookincubator/katran
- libbpf-rs, https://github.com/libbpf/libbpf-rs
- bpfman, https://github.com/bpfman/bpfman
- `crates/lb-l4-xdp/src/lib.rs` (userspace simulation)
- `crates/lb-l4-xdp/ebpf/src/main.rs` (BPF-side stub)
