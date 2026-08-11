//! Pillar 4b-1: end-to-end sanity check that the committed BPF ELF (`src/lb_xdp.bin`) parses via
//! the aya userspace loader without touching the kernel.

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

/// Confirm the ELF declares exactly one `lb_xdp` entry — guards against accidentally shipping an
/// ELF built from an unrelated crate.
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
