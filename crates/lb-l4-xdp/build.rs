//! Detect whether scripts/build-xdp.sh has produced src/lb_xdp.bin and expose that via the `lb_xdp_elf` cfg, so the loader can `include_bytes!` it without a hard file dependency.
//!
//! EBPF-2-01: the committed ELF is the load-bearing artefact, so a 64 KiB ceiling is enforced here to stop unbounded size drift. `tests/elf_sections.rs` re-asserts the same invariant.

/// EBPF-2-01: ELF size ceiling. Sync with `crates/lb-l4-xdp/tests/elf_sections.rs::MAX_ELF_BYTES`.
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
            // Refuse to emit the cfg: an oversized ELF means the eBPF source grew without a deliberate ceiling bump, and the proof test would reject it anyway.
            panic!(
                "lb_xdp.bin size {size} bytes exceeds MAX_ELF_BYTES ({MAX_ELF_BYTES}); \
                 see EBPF-2-01 budget guard. Either trim the eBPF source or \
                 bump MAX_ELF_BYTES in build.rs AND tests/elf_sections.rs"
            );
        }
        println!("cargo:rustc-cfg=lb_xdp_elf");
    }
}
