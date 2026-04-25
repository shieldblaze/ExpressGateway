//! Build helper: detect whether scripts/build-xdp.sh has produced
//! src/lb_xdp.bin and expose that via the `lb_xdp_elf` cfg, so the loader
//! can optionally `include_bytes!` it without a hard file dependency.
fn main() {
    // Tell cargo this cfg is expected (rustc check-cfg hygiene).
    println!("cargo:rustc-check-cfg=cfg(lb_xdp_elf)");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let elf_path = format!("{manifest_dir}/src/lb_xdp.bin");
    println!("cargo:rerun-if-changed={elf_path}");
    if std::path::Path::new(&elf_path).exists() {
        println!("cargo:rustc-cfg=lb_xdp_elf");
    }
}
