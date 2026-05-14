//! SEC-2-12 proof: the loader's belt-and-suspenders license check
//! must refuse an ELF that lacks a `license` section, and must
//! refuse an ELF whose `license` section payload is not `"GPL\0"`.
//!
//! Both assertions live in
//! [`lb_l4_xdp::loader::XdpLoader::load_from_bytes`] (which delegates
//! to `load_from_bytes_pinned`). The integration test exercises the
//! public entry-point — the corresponding unit test in
//! `src/loader.rs` calls the private `assert_license_is_gpl` helper
//! directly with hand-crafted ELFs.
//!
//! Linux-only: the loader module itself is gated on `target_os =
//! "linux"` because aya talks to the BPF syscall. On non-Linux this
//! file compiles as an empty module.

#![cfg(target_os = "linux")]

use lb_l4_xdp::loader::{XdpLoader, XdpLoaderError};

/// SEC-2-12: an ELF without a `license` section is rejected.
///
/// We pass a 16-byte zero buffer — not a valid ELF, so the
/// `object` parser short-circuits inside the license check and
/// surfaces `LicenseInvalid` with a parse-failure message. The
/// crucial property is the error variant: a regression that
/// removes the license check would silently fall through to aya,
/// which returns `EbpfError`/`Load(_)` instead.
#[test]
fn test_loader_refuses_elf_without_license() {
    let garbage = [0u8; 16];
    let result = XdpLoader::load_from_bytes(&garbage);
    assert!(
        matches!(result, Err(XdpLoaderError::LicenseInvalid(_))),
        "loader must surface LicenseInvalid before aya's Load error; got {result:?}",
    );
}

/// SEC-2-12: a "real" looking ELF (valid header) but no `license`
/// section is also rejected with a message that names the missing
/// section, so operators can fix the build.
#[test]
fn test_loader_refuses_real_elf_without_license_section() {
    // Minimal 64-bit LSB BPF ELF, no sections. `object::File::parse`
    // accepts it; `section_by_name("license")` returns None.
    let mut elf = vec![0u8; 64];
    elf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    elf[4] = 2; // ELFCLASS64
    elf[5] = 1; // ELFDATA2LSB
    elf[6] = 1; // EV_CURRENT
    elf[16..18].copy_from_slice(&1u16.to_le_bytes()); // e_type = ET_REL
    elf[18..20].copy_from_slice(&247u16.to_le_bytes()); // e_machine = EM_BPF
    elf[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    elf[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize

    let result = XdpLoader::load_from_bytes(&elf);
    match result {
        Err(XdpLoaderError::LicenseInvalid(msg)) => {
            assert!(
                msg.to_lowercase().contains("license"),
                "diagnostic must name the missing section, got: {msg}",
            );
        }
        other => panic!("expected LicenseInvalid, got {other:?}"),
    }
}
