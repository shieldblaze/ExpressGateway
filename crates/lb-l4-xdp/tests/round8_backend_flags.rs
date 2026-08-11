//! ROUND8-L4-07 proof: the legacy `flags: u32` field is gone from both `BackendEntry` and
//! `BackendEntryV6`.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lb_l4_xdp::loader::{BACKEND_ENTRY_SIZE, BACKEND_ENTRY_V6_SIZE, BackendEntry, BackendEntryV6};

const MAC_A: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const MAC_B: [u8; 6] = [0x02, 0, 0, 0, 0, 2];

#[test]
fn backend_entry_size_is_24_post_l4_07() {
    assert_eq!(
        BACKEND_ENTRY_SIZE, 24,
        "BACKEND_ENTRY_SIZE must be 24 after L4-07 dropped the flags field"
    );
    assert_eq!(core::mem::size_of::<BackendEntry>(), 24);
}

#[test]
fn backend_entry_v6_size_is_36_post_l4_07() {
    assert_eq!(
        BACKEND_ENTRY_V6_SIZE, 36,
        "BACKEND_ENTRY_V6_SIZE must be 36 after L4-07 dropped the flags field"
    );
    assert_eq!(core::mem::size_of::<BackendEntryV6>(), 36);
}

#[test]
fn backend_entry_constructor_takes_no_flags_arg() {
    let _: BackendEntry = BackendEntry::new(1, 0x0A00_0001, 8080, MAC_A, MAC_B);
}

#[test]
fn backend_entry_v6_constructor_takes_no_flags_arg() {
    let mut ip = [0u8; 16];
    ip[15] = 1;
    let _: BackendEntryV6 = BackendEntryV6::new(2, ip, 8080, MAC_A, MAC_B);
}

#[test]
fn no_flags_field_on_backend_entry() {
    let e = BackendEntry::new(7, 0x0A00_0001, 8080, MAC_A, MAC_B);
    let _idx = e.backend_idx;
    let _ip = e.backend_ip;
    let _port = e.backend_port;
    let _pad = e.pad;
    let _bmac = e.backend_mac;
    let _smac = e.src_mac;
    let field_byte_sum = core::mem::size_of_val(&_idx)
        + core::mem::size_of_val(&_ip)
        + core::mem::size_of_val(&_port)
        + core::mem::size_of_val(&_pad)
        + core::mem::size_of_val(&_bmac)
        + core::mem::size_of_val(&_smac);
    assert_eq!(field_byte_sum, BACKEND_ENTRY_SIZE);
}
