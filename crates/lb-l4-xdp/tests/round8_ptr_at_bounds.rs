//! ROUND8-L4-09 proof: the `ptr_at` checked-arithmetic bounds check rejects wrap-around offsets and
//! accepts the in-bounds cases.

/// Userspace mirror of `crates/lb-l4-xdp/ebpf/src/main.rs` `ptr_at` arithmetic.
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
    assert!(!ptr_at_in_bounds(usize::MAX, 1, 0, usize::MAX));
    assert!(!ptr_at_in_bounds(usize::MAX - 10, 100, 0, usize::MAX));
}

#[test]
fn rejects_len_wraparound() {
    assert!(!ptr_at_in_bounds(0, usize::MAX, 1, usize::MAX));
    assert!(!ptr_at_in_bounds(100, usize::MAX - 50, 100, usize::MAX));
}

#[test]
fn rejects_out_of_bounds_no_wrap() {
    assert!(!ptr_at_in_bounds(0, 100, 50, 100));
    assert!(!ptr_at_in_bounds(0, 0, 200, 100));
}

#[test]
fn accepts_in_bounds() {
    assert!(ptr_at_in_bounds(0x1000, 14, 20, 0x1000 + 1500));
    assert!(ptr_at_in_bounds(0, 50, 50, 100));
    assert!(ptr_at_in_bounds(0, 0, 0, 0));
}

#[test]
fn rejects_strict_greater_than_end() {
    assert!(!ptr_at_in_bounds(0, 50, 51, 100));
}

#[test]
fn header_size_corpus_in_bounds() {
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
    let start = 0_usize;
    let end = 16_384_usize; // a generous packet buffer
    let worst_case_off = 4_136_usize;
    assert!(ptr_at_in_bounds(start, worst_case_off, 20, end));
}
