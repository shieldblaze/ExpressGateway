//! Mirror of `crates/lb-l4-xdp/build.rs`: emit `cfg(lb_xdp_elf)` when the
//! compiled BPF ELF is present, so `#[cfg(lb_xdp_elf)]` gates inside
//! `crates/lb/src/xdp.rs` work the same way they do in the lb-l4-xdp
//! crate. Cargo cfg values do not propagate across crates, so each
//! consumer that wants to fence on ELF availability re-runs the check.
fn main() {
    println!("cargo:rustc-check-cfg=cfg(lb_xdp_elf)");
    let elf_path = format!(
        "{}/../lb-l4-xdp/src/lb_xdp.bin",
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()
    );
    println!("cargo:rerun-if-changed={elf_path}");
    if std::path::Path::new(&elf_path).exists() {
        println!("cargo:rustc-cfg=lb_xdp_elf");
    }
}
