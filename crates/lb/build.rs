//! Emit `cfg(lb_xdp_elf)` when the compiled BPF ELF is present. Cargo cfg values do NOT
//! propagate across crates, so this check is duplicated per consumer (see lb-l4-xdp/build.rs).
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
