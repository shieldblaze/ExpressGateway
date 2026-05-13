//! Build helper: detect whether scripts/build-xdp.sh has produced
//! src/lb_xdp.bin and expose that via the `lb_xdp_elf` cfg, so the loader
//! can optionally `include_bytes!` it without a hard file dependency.
//!
//! EBPF-2-01 size-budget guard: the committed ELF is the load-bearing
//! artefact (we don't rebuild on every consumer's machine). Enforcing a
//! 64 KiB ceiling prevents unbounded size drift when future eBPF source
//! changes pull in more BTF or more code. The current post-fix size is
//! ~10-16 KiB; 64 KiB gives ~4x headroom and forces a deliberate
//! review on any future growth. The proof test
//! (`tests/elf_sections.rs`) re-asserts the same invariant at
//! `cargo test` time.

/// EBPF-2-01: ELF size ceiling. Sync with
/// `crates/lb-l4-xdp/tests/elf_sections.rs::MAX_ELF_BYTES`.
const MAX_ELF_BYTES: u64 = 64 * 1024;

fn main() {
    // Tell cargo this cfg is expected (rustc check-cfg hygiene).
    println!("cargo:rustc-check-cfg=cfg(lb_xdp_elf)");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let elf_path = format!("{manifest_dir}/src/lb_xdp.bin");
    println!("cargo:rerun-if-changed={elf_path}");
    if let Ok(meta) = std::fs::metadata(&elf_path) {
        let size = meta.len();
        if size > MAX_ELF_BYTES {
            // Hard-fail the build: an oversized ELF means the eBPF source
            // grew without a corresponding ceiling bump. Refuse to emit
            // the `lb_xdp_elf` cfg so downstream consumers don't include
            // a binary the proof test would reject anyway.
            panic!(
                "lb_xdp.bin size {size} bytes exceeds MAX_ELF_BYTES ({MAX_ELF_BYTES}); \
                 see EBPF-2-01 budget guard. Either trim the eBPF source or \
                 bump MAX_ELF_BYTES in build.rs AND tests/elf_sections.rs"
            );
        }
        println!("cargo:rustc-cfg=lb_xdp_elf");
    }
}
