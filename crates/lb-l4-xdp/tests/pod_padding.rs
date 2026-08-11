//! CODE-2-07 proof: the Pod constructors MUST zero-initialise their padding bytes.
//!
//! The hazard: a future contributor reaching for `MaybeUninit::uninit().assume_init()` would publish arbitrary stack bytes through aya into the BPF map — leaking uninit memory to the kernel AND desyncing the hash key, because the verifier treats the padding as part of the lookup key. Each test transmutes a `::new(...)` value to bytes and asserts the pad is zero.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lb_l4_xdp::loader::{
    BACKEND_ENTRY_SIZE, BACKEND_ENTRY_V6_SIZE, BackendEntry, BackendEntryV6, FLOWKEY_SIZE,
    FLOWKEY_V6_SIZE, FlowKey, FlowKeyV6,
};

/// Transmute a `Copy` Pod-shaped value to its bytes.
/// SAFETY: every type passed here is `#[repr(C)] + Copy + Pod`; the
/// caller statically guarantees `N == size_of::<T>()` (we check that
/// at the call-site via `assert_eq!`).
fn to_bytes<T: Copy, const N: usize>(value: T) -> [u8; N] {
    assert_eq!(N, core::mem::size_of::<T>(), "size mismatch in test helper");
    // SAFETY: `T: Copy` + `#[repr(C)]` (every caller is one of the
    // four Pod types) and `N == size_of::<T>()` (asserted above).
    unsafe { core::mem::transmute_copy(&value) }
}

#[test]
fn test_flowkey_pad_zeroed_after_new() {
    // Every byte except the pad region is set to 0xFF so pad is distinguishable.
    let k = FlowKey::new(
        0xFFFF_FFFF, // src_addr
        0xFFFF,      // src_port
        0xFFFF_FFFF, // dst_addr
        0xFFFF,      // dst_port
        0xFF,        // protocol
    );
    let bytes: [u8; FLOWKEY_SIZE] = to_bytes(k);

    // Layout: src(4) | dst(4) | sport(2) | dport(2) | proto(1) | pad(3); pad lives at bytes [13..16].
    assert_eq!(
        &bytes[13..16],
        &[0u8; 3],
        "FlowKey::new must zero-initialise the trailing 3-byte pad; \
         got pad={:?} (full bytes={:?})",
        &bytes[13..16],
        bytes
    );
    // Field bytes still 0xFF — sanity that `to_bytes` did not swap the layout under us.
    assert!(
        bytes[..13].iter().all(|&b| b == 0xFF),
        "non-pad bytes must mirror constructor args, got {bytes:?}"
    );
}

#[test]
fn test_flowkeyv6_pad_zeroed_after_new() {
    let k = FlowKeyV6::new(
        [0xFF; 16], // src_addr
        0xFFFF,     // src_port
        [0xFF; 16], // dst_addr
        0xFFFF,     // dst_port
        0xFF,       // protocol
    );
    let bytes: [u8; FLOWKEY_V6_SIZE] = to_bytes(k);

    // Layout: src(16) | dst(16) | sport(2) | dport(2) | proto(1) | pad(3); pad lives at bytes [37..40].
    assert_eq!(
        &bytes[37..40],
        &[0u8; 3],
        "FlowKeyV6::new must zero-initialise the trailing 3-byte pad; \
         got pad={:?} (full bytes={:?})",
        &bytes[37..40],
        bytes
    );
    assert!(
        bytes[..37].iter().all(|&b| b == 0xFF),
        "non-pad bytes must mirror constructor args"
    );
}

#[test]
fn test_backend_entry_pad_zeroed_after_new() {
    // ROUND8-L4-07 layout post-flags-removal: idx(4) | bip(4) | bport(2) | pad(2) | bmac(6) | smac(6) = 24; pad lives at bytes [10..12].
    let v = BackendEntry::new(
        0xFFFF_FFFF, // backend_idx
        0xFFFF_FFFF, // backend_ip
        0xFFFF,      // backend_port
        [0xFF; 6],   // backend_mac
        [0xFF; 6],   // src_mac
    );
    let bytes: [u8; BACKEND_ENTRY_SIZE] = to_bytes(v);

    assert_eq!(
        &bytes[10..12],
        &[0u8; 2],
        "BackendEntry::new must zero-initialise the 2-byte pad between \
         backend_port and backend_mac; got pad={:?} (full bytes={:?})",
        &bytes[10..12],
        bytes
    );
    assert!(bytes[..10].iter().all(|&b| b == 0xFF));
    assert!(bytes[12..].iter().all(|&b| b == 0xFF));
}

#[test]
fn test_backend_entry_v6_pad_zeroed_after_new() {
    // ROUND8-L4-07 layout post-flags-removal: idx(4) | bip(16) | bport(2) | pad(2) | bmac(6) | smac(6) = 36; pad lives at bytes [22..24].
    let v = BackendEntryV6::new(
        0xFFFF_FFFF, // backend_idx
        [0xFF; 16],  // backend_ip
        0xFFFF,      // backend_port
        [0xFF; 6],   // backend_mac
        [0xFF; 6],   // src_mac
    );
    let bytes: [u8; BACKEND_ENTRY_V6_SIZE] = to_bytes(v);

    assert_eq!(
        &bytes[22..24],
        &[0u8; 2],
        "BackendEntryV6::new must zero-initialise the 2-byte pad; \
         got pad={:?} (full bytes={:?})",
        &bytes[22..24],
        bytes
    );
    assert!(bytes[..22].iter().all(|&b| b == 0xFF));
    assert!(bytes[24..].iter().all(|&b| b == 0xFF));
}

/// CODE-2-07: `loader.rs` already const-asserts these sizes, but re-asserting from the test crate catches invocations that bypass the const_assert (e.g. dependency-only builds).
#[test]
fn test_struct_sizes_match_bpf_side() {
    assert_eq!(core::mem::size_of::<FlowKey>(), FLOWKEY_SIZE);
    assert_eq!(core::mem::size_of::<FlowKeyV6>(), FLOWKEY_V6_SIZE);
    assert_eq!(core::mem::size_of::<BackendEntry>(), BACKEND_ENTRY_SIZE);
    assert_eq!(
        core::mem::size_of::<BackendEntryV6>(),
        BACKEND_ENTRY_V6_SIZE
    );
}
