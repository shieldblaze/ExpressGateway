//! Miri harness for lb-io's ring bookkeeping. It models the head/tail arithmetic rather than
//! calling io_uring, because miri cannot model the syscalls.
//!
//! DUAL-MODE by design: it asserts real arithmetic under plain `cargo test -p lb-io`, and catches
//! raw-pointer UB under `cargo +nightly miri test -p lb-io --test miri_ring --
//! -Zmiri-disable-isolation`. It is scaffolding, not exhaustive coverage.

/// SPSC ring math: head==tail is empty, tail-head==N is full.
#[test]
fn head_tail_wrap_arithmetic() {
    const N: u32 = 8;
    let mut head: u32 = 0;
    let mut tail: u32 = 0;

    for i in 0..N {
        let used = tail.wrapping_sub(head);
        assert!(used < N, "ring full at index {i}");
        tail = tail.wrapping_add(1);
    }
    assert_eq!(tail.wrapping_sub(head), N, "ring should now be full");

    for i in 0..N {
        let used = tail.wrapping_sub(head);
        assert!(used > 0, "ring empty at pop {i}");
        head = head.wrapping_add(1);
    }
    assert_eq!(head, tail, "ring should now be empty");
}

/// Past `u32::MAX`, so `wrapping_sub` is actually exercised — miri catches a reliance on
/// non-wrapping overflow here.
#[test]
fn head_tail_wraps_past_u32_max() {
    let mut head: u32 = u32::MAX - 2;
    let mut tail: u32 = u32::MAX - 2;
    for _ in 0..8 {
        let used = tail.wrapping_sub(head);
        assert!(used < 8);
        tail = tail.wrapping_add(1);
        let used2 = tail.wrapping_sub(head);
        assert_eq!(used2, 1);
        head = head.wrapping_add(1);
        assert_eq!(tail.wrapping_sub(head), 0);
    }
}

/// The slice-from-raw-parts pattern lb-io uses for the SQE buffer; miri surfaces provenance or
/// aliasing UB here.
#[test]
fn raw_slice_round_trip_is_provenance_clean() {
    let mut buf = [0u8; 16];
    let len = buf.len();
    let ptr = buf.as_mut_ptr();

    // SAFETY: `ptr` derives from `buf`; len matches; lifetime is
    // bounded by `buf` which is owned by the test stack frame.
    let slice: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
    for (i, b) in slice.iter_mut().enumerate() {
        *b = i as u8;
    }
    // Reading back through the ORIGINAL binding is what proves provenance survived.
    for (i, b) in buf.iter().enumerate() {
        assert_eq!(*b, i as u8);
    }
}
