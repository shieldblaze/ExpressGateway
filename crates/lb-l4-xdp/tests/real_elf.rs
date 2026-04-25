//! Pillar 4b-1: end-to-end sanity check that the committed BPF ELF
//! (`src/lb_xdp.bin`) parses via the aya userspace loader without
//! touching the kernel.
//!
//! Gated on `cfg(lb_xdp_elf)`, emitted by `build.rs` when the ELF file
//! is present. When the toolchain has not yet produced the ELF, this
//! file compiles as an empty module so `cargo test` still acknowledges
//! it.
//!
//! We call [`XdpLoader::program_names`] — the kernel-free path — so CI
//! runs without `CAP_BPF`. Full `XdpLoader::load_from_bytes` creates
//! BPF maps in the kernel and is gated to the Pillar 4b-2 privileged
//! runner.

#![cfg(all(target_os = "linux", lb_xdp_elf))]

use lb_l4_xdp::{LB_XDP_ELF, loader::XdpLoader};

#[test]
fn real_elf_parses_via_loader() {
    let names =
        XdpLoader::program_names(LB_XDP_ELF).expect("committed BPF ELF should parse via aya-obj");
    assert!(
        names.iter().any(|n| n == "lb_xdp"),
        "expected program 'lb_xdp' in parsed ELF, got {names:?}",
    );
}

/// Confirm the ELF declares exactly one `lb_xdp` entry — guards against
/// accidentally shipping an ELF built from an unrelated crate.
#[test]
fn real_elf_has_single_lb_xdp_program() {
    let names = XdpLoader::program_names(LB_XDP_ELF).expect("parse BPF ELF");
    let matches: Vec<_> = names.iter().filter(|n| *n == "lb_xdp").collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one 'lb_xdp' program, got {names:?}",
    );
}
