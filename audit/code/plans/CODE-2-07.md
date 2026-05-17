# Plan for CODE-2-07 + EBPF-2-09 — Pod constructor zero-init invariant
Finding-ref:     CODE-2-07 / EBPF-2-09 (high, Open) — merged per lead §E.5
Files touched:
  - `crates/lb-l4-xdp/src/loader.rs`                (private fields + constructors)
  - `crates/lb-l4-xdp/src/lib.rs`                   (call-site replacements at lines 420,448,478,485,575,598)
  - `crates/lb-l4-xdp/src/sim.rs`                   (call-site replacements at lines 131,312)
  - `crates/lb-l4-xdp/tests/pod_layout.rs`          (NEW — size + miri test)

Approach:
Make the four Pod types' fields private; provide `pub fn new(...)`
that zero-initialises padding; add compile-time size assertions;
replace every struct-literal site with the constructor.

Step 1 — Type changes in `loader.rs`:
```rust
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct FlowKey {
    src_addr: u32,
    dst_addr: u32,
    src_port: u16,
    dst_port: u16,
    protocol: u8,
    pad: [u8; 3],
}
unsafe impl aya::Pod for FlowKey {}

impl FlowKey {
    #[must_use]
    pub fn new(src_addr: u32, dst_addr: u32, src_port: u16, dst_port: u16, protocol: u8) -> Self {
        Self { src_addr, dst_addr, src_port, dst_port, protocol, pad: [0; 3] }
    }
    pub fn src_addr(&self)  -> u32 { self.src_addr }
    pub fn dst_addr(&self)  -> u32 { self.dst_addr }
    pub fn src_port(&self)  -> u16 { self.src_port }
    pub fn dst_port(&self)  -> u16 { self.dst_port }
    pub fn protocol(&self)  -> u8  { self.protocol }
}
const _: () = assert!(core::mem::size_of::<FlowKey>() == 16);
const _: () = assert!(core::mem::align_of::<FlowKey>() == 4);
```
Same shape for `FlowKeyV6` (size 40), `BackendEntry` (size 24),
`BackendEntryV6` (size 40 or 48 depending on alignment of v6 mac
buffer — final number locked by the compile-time assert in Round 4).

Step 2 — Replace every call site. The six call sites in
`lb-l4-xdp/src/lib.rs` and two in `sim.rs` change from
```rust
let k = FlowKey { src_addr: a, dst_addr: b, src_port: c, dst_port: d, protocol: 6, pad: [0; 3] };
```
to
```rust
let k = FlowKey::new(a, b, c, d, 6);
```
The `#[must_use]` on `new` makes silent-discard a clippy warning.
Private fields make struct-literal construction *impossible* outside
the defining module, fully enforcing the invariant.

Step 3 — eBPF side. The eBPF crate's `crates/lb-l4-xdp/ebpf/src/main.rs:427`
is owned by `ebpf` (EBPF-2-09 confirmation slice). This plan only
publishes the userspace size assertion that matches what `ebpf`
confirms:
```rust
// Lock-stepped with crates/lb-l4-xdp/ebpf/src/main.rs:427 struct flow_key { ... }
const _: () = assert!(core::mem::size_of::<FlowKey>() == 16);
```
Any byte-level drift between sides becomes a compile error.

Step 4 — Miri test (paired with CODE-2-11). `tests/pod_layout.rs`:
```rust
#[test]
fn flowkey_pod_roundtrip() {
    let k = FlowKey::new(0x0a000001, 0x0a000002, 80, 443, 6);
    let bytes: [u8; 16] = unsafe { std::mem::transmute(k) };
    let k2: FlowKey = unsafe { std::mem::transmute(bytes) };
    assert_eq!(k, k2);
}
#[test]
fn flowkey_padding_zeroed_by_new() {
    let k = FlowKey::new(0, 0, 0, 0, 6);
    let bytes: [u8; 16] = unsafe { std::mem::transmute(k) };
    assert_eq!(bytes[13..16], [0u8; 3], "padding not zeroed");
}
```
Run under `cargo +nightly miri test -p lb-l4-xdp --test pod_layout`
(MIRI catches any UB in the transmute round-trip).

Proof:
- `cargo test -p lb-l4-xdp --test pod_layout`: covers roundtrip and
  padding-zero invariants.
- `cargo +nightly miri test -p lb-l4-xdp --test pod_layout`:
  validates no UB in the Pod transmute (gated in CI as the miri job
  from CODE-2-11).
- Compile-time `const _: () = assert!(size_of == N)` failures are
  build-stop signals; locks the ABI.
- Private fields produce compile errors at any future struct-literal
  attempt (negative test: a `compile_fail` doctest in `loader.rs`).

Risk / blast radius:
- Private fields require all 8 known call sites to migrate to
  constructors. Mechanical; the compiler tells you each one.
- Size asserts may need adjustment for `BackendEntryV6` based on
  layout; Round 4 finalises the number once the v6 mac/pad sizes are
  re-verified against the eBPF side.
- `#[must_use]` on `new` is a hygiene addition only; existing tests
  bind the result so no warnings.

Cross-ref:    EBPF-2-09 (merged), SEC-2-09 (security framing),
              CODE-2-11 (miri job hosts pod_layout test)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
