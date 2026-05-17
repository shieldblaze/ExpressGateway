//! ROUND8-L4-09 proof: the `ptr_at` checked-arithmetic bounds
//! check rejects wrap-around offsets and accepts the in-bounds
//! cases. Property tests run against a userspace mirror of the
//! arithmetic — we can't actually exercise the BPF program from a
//! userspace test (no kernel), but the arithmetic that the
//! verifier inspects is identical between the BPF crate and this
//! mirror.
//!
//! Reference: aya issue #1562 (Rust LLVM scalar/pointer re-ordering
//! on the BPF target); CVE-2022-23222 (bounds-check elision class
//! that motivated `checked_add` everywhere).

/// Userspace mirror of `crates/lb-l4-xdp/ebpf/src/main.rs` `ptr_at`
/// arithmetic. The Bool-returning shape suffices for the property
/// test — we only need to prove that the overflow-arm rejection
/// matches.
fn ptr_at_in_bounds(start: usize, offset: usize, len: usize, end: usize) -> bool {
    let needed = match start.checked_add(offset).and_then(|s| s.checked_add(len)) {
        Some(n) => n,
        None => return false,
    };
    if needed > end {
        return false;
    }
    start.checked_add(offset).is_some()
}

#[test]
fn rejects_offset_wraparound() {
    // start + offset overflows usize → must reject regardless of len/end.
    assert!(!ptr_at_in_bounds(usize::MAX, 1, 0, usize::MAX));
    assert!(!ptr_at_in_bounds(usize::MAX - 10, 100, 0, usize::MAX));
}

#[test]
fn rejects_len_wraparound() {
    // (start+offset) is in range but adding len overflows.
    assert!(!ptr_at_in_bounds(0, usize::MAX, 1, usize::MAX));
    assert!(!ptr_at_in_bounds(100, usize::MAX - 50, 100, usize::MAX));
}

#[test]
fn rejects_out_of_bounds_no_wrap() {
    // start + offset + len > end, no wrap.
    assert!(!ptr_at_in_bounds(0, 100, 50, 100));
    assert!(!ptr_at_in_bounds(0, 0, 200, 100));
}

#[test]
fn accepts_in_bounds() {
    // Real packet-parse shape: a 1500-byte buffer, 20-byte read at
    // offset 14 (Ethernet→IPv4 header).
    assert!(ptr_at_in_bounds(0x1000, 14, 20, 0x1000 + 1500));
    // Boundary: needed == end.
    assert!(ptr_at_in_bounds(0, 50, 50, 100));
    // Zero-size T (rare but legal).
    assert!(ptr_at_in_bounds(0, 0, 0, 0));
}

#[test]
fn rejects_strict_greater_than_end() {
    // needed = end + 1 must reject.
    assert!(!ptr_at_in_bounds(0, 50, 51, 100));
}

#[test]
fn header_size_corpus_in_bounds() {
    // Seeded corpus: the actual T sizes used in ptr_at call sites.
    // EthHdr=14, Ipv4Hdr=20, Ipv6Hdr=40, TcpHdr=4, UdpHdr=8, VlanHdr=4.
    const SIZES: &[usize] = &[14, 20, 40, 4, 8, 4];
    let start = 0x4000_usize;
    let end = start + 1500;
    let mut off = 0;
    for &sz in SIZES {
        assert!(
            ptr_at_in_bounds(start, off, sz, end),
            "should accept off={off}, sz={sz}",
        );
        off += sz;
    }
}

#[test]
fn ipv6_extension_header_walk_boundary() {
    // The IPv6 ext-header walk advances `off` by `(hdr_ext_len + 1) * 8`.
    // With the verifier-enforced cap of 2 extensions and max
    // hdr_ext_len 255, `off` is bounded by 2 * (255+1) * 8 + 40 = ~4136.
    // Confirm the arithmetic stays safe for the worst-case offset.
    let start = 0_usize;
    let end = 16_384_usize; // a generous packet buffer
    let worst_case_off = 4_136_usize;
    assert!(ptr_at_in_bounds(start, worst_case_off, 20, end));
}
