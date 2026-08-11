//! ROUND8-L4-01 proof: `BackendEntry::try_new` / `BackendEntryV6::try_new` reject the zero-IP /
//! zero-port sentinels that cause a silent `XDP_TX` to 0.0.0.0:0 (Katran lesson 10). The
//! eBPF-side guard is the runtime mirror; this covers the construction-time admission gate.

#![cfg(target_os = "linux")]

use lb_l4_xdp::loader::{BackendEntry, BackendEntryV6, XdpLoaderError};
use lb_l4_xdp::stats_export::{NUM_SLOTS, StatSlot};

const MAC_A: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const MAC_B: [u8; 6] = [0x02, 0, 0, 0, 0, 2];

#[test]
fn userspace_try_new_rejects_zero_ip_v4() {
    let r = BackendEntry::try_new(7, 0, 8080, MAC_A, MAC_B);
    match r {
        Err(XdpLoaderError::BackendUnpopulated { reason }) => {
            assert!(
                reason.contains("backend_ip"),
                "error reason must mention backend_ip; got: {reason}",
            );
        }
        other => panic!("expected BackendUnpopulated, got {other:?}"),
    }
}

#[test]
fn userspace_try_new_rejects_zero_port_v4() {
    let r = BackendEntry::try_new(7, 0x0A0A_0A0A, 0, MAC_A, MAC_B);
    match r {
        Err(XdpLoaderError::BackendUnpopulated { reason }) => {
            assert!(reason.contains("backend_port"));
        }
        other => panic!("expected BackendUnpopulated, got {other:?}"),
    }
}

#[test]
fn userspace_try_new_accepts_populated_v4() {
    let r = BackendEntry::try_new(7, 0x0A0A_0A0A, 8080, MAC_A, MAC_B);
    assert!(r.is_ok(), "populated entry must succeed: {r:?}");
}

#[test]
fn userspace_try_new_rejects_zero_ip_v6() {
    let r = BackendEntryV6::try_new(7, [0u8; 16], 8080, MAC_A, MAC_B);
    match r {
        Err(XdpLoaderError::BackendUnpopulated { reason }) => {
            assert!(reason.contains("IPv6 unspecified"));
        }
        other => panic!("expected BackendUnpopulated, got {other:?}"),
    }
}

#[test]
fn userspace_try_new_rejects_zero_port_v6() {
    let mut ip = [0u8; 16];
    ip[15] = 1; // ::1
    let r = BackendEntryV6::try_new(7, ip, 0, MAC_A, MAC_B);
    assert!(matches!(r, Err(XdpLoaderError::BackendUnpopulated { .. })));
}

#[test]
fn legacy_new_still_accepts_zero_for_back_compat() {
    // `new` is the legacy infallible constructor and MUST still accept the zero shapes; the
    // eBPF runtime guard is the load-bearing defence for any production caller.
    let entry = BackendEntry::new(0, 0, 0, MAC_A, MAC_B);
    assert_eq!(entry.backend_ip, 0);
    assert_eq!(entry.backend_port, 0);
}

#[test]
fn stats_slot_backend_unpopulated_at_slot_10() {
    // The eBPF-side `STAT_BACKEND_UNPOPULATED = 10` constant must
    // match the userspace `StatSlot::BackendUnpopulated` discriminant.
    // Drift here corrupts every operator's `xdp_packets_total{result}`
    // labels.
    assert_eq!(StatSlot::BackendUnpopulated as usize, 10);
    // Deliberately loose (`> 10`) so appending slots cannot break this test.
    let slots = NUM_SLOTS;
    assert!(
        slots > 10,
        "NUM_SLOTS={slots} must include the BackendUnpopulated slot",
    );
}
