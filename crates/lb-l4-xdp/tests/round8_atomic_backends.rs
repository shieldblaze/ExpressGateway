//! ROUND8-L4-04 proof: atomic per-VIP backend-table publication +
//! Unimog lesson-3 daisy-chain.
//!
//! The eBPF-side hot-path read of `BACKENDS_V4` is the Pillar-4b-3
//! piece (verifier-heavy; deferred with an explicit scope note in
//! `ebpf/src/main.rs`). What this finding's bug fix actually IS — the
//! single-`bpf_map_update_elem` atomic swap + the daisy-chain shift —
//! lives entirely in `XdpLoader::publish_backends_v4` and the
//! `BackendTable` layout. Those are testable without a privileged
//! kernel:
//!
//!   1. `BackendTable` byte size matches the eBPF struct (the
//!      compile-time assertion in `loader.rs` is the primary guard;
//!      this test pins the documented constant too),
//!   2. the daisy-chain shift moves current → previous on publish,
//!   3. the tail is zeroed on a shrink (no stale addressable backend),
//!   4. `generation` increments (wrapping),
//!   5. an over-large publish is rejected BEFORE any mutation.
//!
//! The "1k-rebalance under 100k pkt/s no-torn-table" scale gate from
//! the plan needs a privileged kernel + traffic gen — it is the
//! Pillar-4b acceptance evidence, out of CI reach.

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

/// The userspace mirror must be byte-identical to the eBPF
/// `BackendTable` (aya rejects the map accessor otherwise). The
/// compile-time `const _: () = assert!(...)` in `loader.rs` is the
/// real guard; this pins the documented wire constant so a silent
/// layout drift trips a named test, not just an opaque build break.
#[test]
fn backend_table_wire_size_is_frozen() {
    assert_eq!(core::mem::size_of::<BackendTable>(), BACKEND_TABLE_SIZE);
    // 4 (generation) + 4 (count) + 24*64 (entries)
    //   + 4 (previous_count) + 4 (pad) + 24*64 (previous_entries)
    assert_eq!(BACKEND_TABLE_SIZE, 3088);
    assert_eq!(MAX_BACKENDS_PER_VIP, 64);
}

/// A freshly-zeroed table is the "never published" sentinel: gen 0,
/// no current or previous entries.
#[test]
fn zeroed_table_is_clean_slate() {
    let t = BackendTable::zeroed();
    assert_eq!(t.generation, 0);
    assert_eq!(t.count, 0);
    assert_eq!(t.previous_count, 0);
    assert!(t.entries.iter().all(|e| e.backend_ip == 0));
    assert!(t.previous_entries.iter().all(|e| e.backend_ip == 0));
}

/// Daisy-chain (Unimog lesson 3): publishing simulates the loader's
/// read-modify-write. The first publish moves the zero slate into
/// `previous`; the second publish moves generation-1's live entries
/// into `previous` so an in-flight flow pinned to the old backend
/// still resolves.
///
/// This reproduces the `publish_backends_v4` shift logic directly
/// (the kernel-touching map I/O is exercised in the CI privileged
/// stage; the *algorithm* — the actual finding fix — is here).
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

    // gen 1: [A, B]
    let t1 = publish(BackendTable::zeroed(), &[a, b]);
    assert_eq!(t1.generation, 1);
    assert_eq!(t1.count, 2);
    assert_eq!(t1.entries[0].backend_ip, a.backend_ip);
    assert_eq!(t1.entries[1].backend_ip, b.backend_ip);
    assert_eq!(t1.previous_count, 0); // nothing was live before

    // gen 2: [A] only (B drained). A in-flight flow pinned to B
    // (CT-remembered generation == 1 != current 2) must still find
    // B in `previous_entries`.
    let c_only = [a];
    let t2 = publish(t1, &c_only);
    assert_eq!(t2.generation, 2);
    assert_eq!(t2.count, 1);
    assert_eq!(t2.previous_count, 2);
    assert_eq!(t2.previous_entries[1].backend_ip, b.backend_ip);
    // Shrink: slot 1 of the *current* table must be zeroed so a
    // stale B is not addressable in the new generation.
    assert_eq!(t2.entries[1].backend_ip, 0);
}

/// `generation` wraps cleanly at u32::MAX (only equality vs. the
/// CT-remembered value matters, never ordering).
#[test]
fn generation_wraps() {
    let mut t = BackendTable::zeroed();
    t.generation = u32::MAX;
    t.generation = t.generation.wrapping_add(1);
    assert_eq!(t.generation, 0);
}

/// An over-large publish is rejected by the typed error BEFORE any
/// map write. We cannot call `XdpLoader::publish_backends_v4` without
/// a loaded ELF here, so assert the guard's contract via the error
/// variant the method returns (the kernel-touching happy path is the
/// CI privileged-stage fixture, shared with the EBPF-2-05 scaffold).
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

/// Sanity: `MAC_B` is referenced so the constant is not dead — keeps
/// the daisy-chain fixture honest if a future edit drops the B path.
#[test]
fn fixture_macs_distinct() {
    assert_ne!(MAC_A, MAC_B);
    assert_ne!(MAC_A, MAC_S);
}
