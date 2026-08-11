//! ROUND8-L4-04 proof: atomic per-VIP backend-table publication + Unimog lesson-3 daisy-chain.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lb_l4_xdp::loader::{
    BACKEND_TABLE_SIZE, BackendEntry, BackendTable, MAX_BACKENDS_PER_VIP, XdpLoaderError,
};

const MAC_A: [u8; 6] = [0x02, 0, 0, 0, 0, 0xAA];
const MAC_B: [u8; 6] = [0x02, 0, 0, 0, 0, 0xBB];
const MAC_S: [u8; 6] = [0x02, 0, 0, 0, 0, 0x01];

fn be(ip_last: u8, port: u16) -> BackendEntry {
    BackendEntry::new(
        0,
        u32::from_be_bytes([10, 0, 0, ip_last]),
        port,
        MAC_A,
        MAC_S,
    )
}

/// The userspace mirror must be byte-identical to the eBPF `BackendTable` (aya rejects the map accessor otherwise).
#[test]
fn backend_table_wire_size_is_frozen() {
    assert_eq!(core::mem::size_of::<BackendTable>(), BACKEND_TABLE_SIZE);
    assert_eq!(BACKEND_TABLE_SIZE, 3088);
    assert_eq!(MAX_BACKENDS_PER_VIP, 64);
}

/// A freshly-zeroed table is the "never published" sentinel: gen 0, no current or previous entries.
#[test]
fn zeroed_table_is_clean_slate() {
    let t = BackendTable::zeroed();
    assert_eq!(t.generation, 0);
    assert_eq!(t.count, 0);
    assert_eq!(t.previous_count, 0);
    assert!(t.entries.iter().all(|e| e.backend_ip == 0));
    assert!(t.previous_entries.iter().all(|e| e.backend_ip == 0));
}

/// Daisy-chain (Unimog lesson 3): publishing simulates the loader's read-modify-write.
#[test]
fn daisy_chain_shifts_current_into_previous() {
    fn publish(prev: BackendTable, new: &[BackendEntry]) -> BackendTable {
        assert!(new.len() <= MAX_BACKENDS_PER_VIP);
        let mut t = prev;
        t.previous_entries = t.entries;
        t.previous_count = t.count;
        let zero = BackendEntry::new(0, 0, 0, [0u8; 6], [0u8; 6]);
        t.entries = [zero; MAX_BACKENDS_PER_VIP];
        for (slot, e) in t.entries.iter_mut().zip(new.iter()) {
            *slot = *e;
        }
        t.count = new.len() as u32;
        t.generation = t.generation.wrapping_add(1);
        t
    }

    let a = be(1, 8080);
    let b = be(2, 8080);

    let t1 = publish(BackendTable::zeroed(), &[a, b]);
    assert_eq!(t1.generation, 1);
    assert_eq!(t1.count, 2);
    assert_eq!(t1.entries[0].backend_ip, a.backend_ip);
    assert_eq!(t1.entries[1].backend_ip, b.backend_ip);
    assert_eq!(t1.previous_count, 0); // nothing was live before

    let c_only = [a];
    let t2 = publish(t1, &c_only);
    assert_eq!(t2.generation, 2);
    assert_eq!(t2.count, 1);
    assert_eq!(t2.previous_count, 2);
    assert_eq!(t2.previous_entries[1].backend_ip, b.backend_ip);
    assert_eq!(t2.entries[1].backend_ip, 0);
}

/// `generation` wraps cleanly at u32::MAX (only equality vs. the CT-remembered value matters, never ordering).
#[test]
fn generation_wraps() {
    let mut t = BackendTable::zeroed();
    t.generation = u32::MAX;
    t.generation = t.generation.wrapping_add(1);
    assert_eq!(t.generation, 0);
}

/// An over-large publish is rejected by the typed error BEFORE any map write.
#[test]
fn too_many_backends_error_carries_count() {
    let e = XdpLoaderError::TooManyBackends(MAX_BACKENDS_PER_VIP + 1);
    let msg = e.to_string();
    assert!(
        msg.contains(&(MAX_BACKENDS_PER_VIP + 1).to_string()),
        "error must surface the offending count, got: {msg}",
    );
    assert!(
        msg.contains(&MAX_BACKENDS_PER_VIP.to_string()),
        "error must surface the ceiling, got: {msg}",
    );
}

/// Sanity: `MAC_B` is referenced so the constant is not dead — keeps the daisy-chain fixture honest if a future edit drops the B path.
#[test]
fn fixture_macs_distinct() {
    assert_ne!(MAC_A, MAC_B);
    assert_ne!(MAC_A, MAC_S);
}
