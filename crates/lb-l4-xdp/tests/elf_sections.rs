//! EBPF-2-01 / EBPF-2-02 proof: the committed BPF ELF must carry a `license` section spelling
//! exactly GPL + NUL, must carry non-empty `.BTF` and `.BTF.ext` sections, and must stay under
//! the 64 KiB ceiling `build.rs` enforces. The assertions are strict by default so a stale ELF
//! is caught the moment someone ships without a rebuild.
//!
//! Gated on `cfg(lb_xdp_elf)`: when the ELF is absent this file compiles as an empty module.

#![cfg(all(target_os = "linux", lb_xdp_elf))]

use lb_l4_xdp::LB_XDP_ELF;
use object::{Object, ObjectSection};

/// Sync with `build.rs::MAX_ELF_BYTES`.
const MAX_ELF_BYTES: u64 = 64 * 1024;

#[test]
fn license_section_says_gpl() {
    let elf = object::File::parse(LB_XDP_ELF).expect("parse committed BPF ELF");
    let section = elf
        .section_by_name("license")
        .expect("BPF ELF must declare a `license` section — see EBPF-2-01");
    let data = section.data().expect("read `license` section data");
    assert_eq!(
        data, b"GPL\0",
        "BPF ELF `license` section must be the C-string \"GPL\\0\" \
         (kernel `bpf_attr.license` requires NUL-terminated). Got: {data:?}",
    );
}

#[test]
fn btf_sections_present_and_non_empty() {
    let elf = object::File::parse(LB_XDP_ELF).expect("parse committed BPF ELF");
    let btf = elf
        .section_by_name(".BTF")
        .expect("BPF ELF must declare `.BTF` — see EBPF-2-01");
    assert!(
        btf.size() > 0,
        ".BTF section present but empty; bpf-linker must have emitted \
         it without DWARF input (rebuild scripts/build-xdp.sh with \
         RUSTFLAGS=\"-Cdebuginfo=2\")",
    );
    let btf_ext = elf
        .section_by_name(".BTF.ext")
        .expect("BPF ELF must declare `.BTF.ext` — see EBPF-2-01");
    assert!(btf_ext.size() > 0, ".BTF.ext section present but empty");
}

#[test]
fn elf_size_within_budget() {
    // build.rs hard-fails over MAX_ELF_BYTES, but cargo runs build.rs and tests in different
    // processes — repeat it here so a stale `target/` still catches the regression.
    let elf_len = LB_XDP_ELF.len() as u64;
    assert!(
        elf_len <= MAX_ELF_BYTES,
        "lb_xdp.bin is {elf_len} bytes — exceeds MAX_ELF_BYTES ({MAX_ELF_BYTES}); \
         see EBPF-2-01 budget guard. Either trim the eBPF source or \
         bump MAX_ELF_BYTES in build.rs and tests/elf_sections.rs in lock-step.",
    );
}
