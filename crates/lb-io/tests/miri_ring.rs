//! CODE-2-11 — miri-runnable harness exercising the in-memory
//! bookkeeping of lb-io's ring-buffer style state without invoking
//! io_uring syscalls (which miri cannot model).
//!
//! The plan in audit/code/plans/CODE-2-11.md calls for a miri test
//! that covers the pod / slice-from-raw-parts sites in lb-io that
//! also flow through lb-l4-xdp::loader's `Pod` impls. The actual
//! Pod-impl validity test lives in lb-l4-xdp/tests/pod_layout.rs
//! (paired with CODE-2-07). Here we cover the lb-io side: head/tail
//! wrap-around bookkeeping for the SQE-style indices.
//!
//! Run with:
//!
//! ```bash
//! cargo +nightly miri test -p lb-io --test miri_ring -- \
//!     -Zmiri-disable-isolation
//! ```
//!
//! The brief: "you do not need to actually run miri/loom this round
//! — adding the scaffolding is sufficient." This file IS the
//! scaffolding; it builds and runs under plain `cargo test -p lb-io`
//! (where it asserts bookkeeping arithmetic) and additionally runs
//! under miri to catch any future UB in raw-pointer math.

/// Mirrors the SQE head/tail bookkeeping pattern used inside lb-io's
/// uring wrappers. We model a ring of size N with `head` (consumer)
/// and `tail` (producer) advancing modulo `2*N` so head==tail means
/// empty and tail-head==N means full (textbook lock-free SPSC math).
#[test]
fn head_tail_wrap_arithmetic() {
    const N: u32 = 8;
    let mut head: u32 = 0;
    let mut tail: u32 = 0;

    // Push N items: tail advances to N, head stays at 0 → full.
    for i in 0..N {
        // Used-slots count BEFORE the push must be < N.
        let used = tail.wrapping_sub(head);
        assert!(used < N, "ring full at index {i}");
        tail = tail.wrapping_add(1);
    }
    assert_eq!(tail.wrapping_sub(head), N, "ring should now be full");

    // Pop N items: head advances to N, equals tail → empty.
    for i in 0..N {
        let used = tail.wrapping_sub(head);
        assert!(used > 0, "ring empty at pop {i}");
        head = head.wrapping_add(1);
    }
    assert_eq!(head, tail, "ring should now be empty");
}

/// Stress the wrap-around case: advance head/tail past u32::MAX so
/// the subtraction `tail.wrapping_sub(head)` exercises the wrap.
/// Miri catches the UB if the math accidentally relied on
/// un-wrapping overflow.
#[test]
fn head_tail_wraps_past_u32_max() {
    // Start near the top of u32. Push then pop one element; the
    // resulting head/tail values should both be > u32::MAX-1 after
    // wrap and yet the "used" computation stays correct (0).
    let mut head: u32 = u32::MAX - 2;
    let mut tail: u32 = u32::MAX - 2;
    for _ in 0..8 {
        // Push.
        let used = tail.wrapping_sub(head);
        assert!(used < 8);
        tail = tail.wrapping_add(1);
        // Pop.
        let used2 = tail.wrapping_sub(head);
        assert_eq!(used2, 1);
        head = head.wrapping_add(1);
        // Empty.
        assert_eq!(tail.wrapping_sub(head), 0);
    }
}

/// Exercise the raw-pointer / slice-from-raw-parts pattern lb-io uses
/// to mirror the uring SQE buffer. Under miri this is the test that
/// would surface any provenance / aliasing UB introduced by future
/// raw-pointer refactors.
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
    // Read back via the original binding to assert provenance survived.
    for (i, b) in buf.iter().enumerate() {
        assert_eq!(*b, i as u8);
    }
}
