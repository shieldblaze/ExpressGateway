//! EBPF-2-01 / EBPF-2-02 proof test: the committed BPF ELF must carry
//! a `license` section spelling exactly `"GPL\0"`, must carry `.BTF`
//! and `.BTF.ext` sections with non-zero size, and must stay under the
//! 64 KiB ceiling enforced by `build.rs`.
//!
//! Gated on `cfg(lb_xdp_elf)` — when the ELF file is absent (e.g. on
//! an aarch64-musl CI image that skips the build-xdp step) this test
//! file compiles as an empty module so `cargo test` still acknowledges
//! it.
//!
//! NOTE on the local sandbox: until `scripts/build-xdp.sh` is re-run
//! against the post-EBPF-2-01 source, the committed ELF still lacks
//! the new sections. CI is responsible for rebuilding and committing
//! the refreshed ELF, after which this test runs green. The
//! assertions are written strict-by-default so a stale ELF is caught
//! the moment a contributor tries to ship without a rebuild.

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
    // build.rs hard-fails over MAX_ELF_BYTES, but cargo runs build.rs
    // and tests in different processes — repeat the assertion here so
    // a manual `cargo test --no-run` past a stale `target/` still
    // catches the regression.
    let elf_len = LB_XDP_ELF.len() as u64;
    assert!(
        elf_len <= MAX_ELF_BYTES,
        "lb_xdp.bin is {elf_len} bytes — exceeds MAX_ELF_BYTES ({MAX_ELF_BYTES}); \
         see EBPF-2-01 budget guard. Either trim the eBPF source or \
         bump MAX_ELF_BYTES in build.rs and tests/elf_sections.rs in lock-step.",
    );
}
