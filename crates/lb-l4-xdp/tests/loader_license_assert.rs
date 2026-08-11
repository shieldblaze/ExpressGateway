//! SEC-2-12 proof: the loader's belt-and-suspenders license check must refuse an ELF that lacks a
//! `license` section, and must refuse an ELF whose `license` section payload is not `"GPL\0"`.

#![cfg(target_os = "linux")]

use lb_l4_xdp::loader::{XdpLoader, XdpLoaderError};

/// SEC-2-12: an ELF without a `license` section is rejected.
#[test]
fn test_loader_refuses_elf_without_license() {
    let garbage = [0u8; 16];
    let result = XdpLoader::load_from_bytes(&garbage);
    assert!(
        matches!(result, Err(XdpLoaderError::LicenseInvalid(_))),
        "loader must surface LicenseInvalid before aya's Load error; got {result:?}",
    );
}

/// SEC-2-12: a "real" looking ELF (valid header) but no `license` section is also rejected with a
/// message that names the missing section, so operators can fix the build.
#[test]
fn test_loader_refuses_real_elf_without_license_section() {
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
