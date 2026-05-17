//! ROUND8-L4-03 proof: per-CPU new-flow-rate cap / SYN-flood
//! mitigation (Katran `is_under_flood()` lesson 4).
//!
//! The eBPF-side `is_under_flood()` gate cannot run in a userspace
//! `cargo test` (no kernel BPF program, no `bpf_ktime_get_ns`). What
//! IS testable without a privileged kernel is the userspace
//! `CtInsertGate` leaky-bucket — the control-plane mirror of the
//! data-plane cap. The kernel-side gate is covered by the verifier
//! matrix + the hping3 scale gate documented in the plan's "Proof"
//! section (Pillar 4b acceptance), out of reach of CI.
//!
//! These tests pin the leaky-bucket invariants the SYN-flood
//! mitigation depends on:
//!   1. a burst above the cap is partially denied,
//!   2. the bucket refills across a window boundary,
//!   3. `cap == 0` disables the gate entirely (operator opt-out).

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lb_l4_xdp::loader::{CtInsertGate, DEFAULT_NEW_FLOW_CAP_PER_SEC_PER_CPU};

/// A burst far above the per-second cap must be partially denied:
/// the gate admits at most `burst` (== refill_per_sec) tokens before
/// the bucket empties. This is the SYN-flood lever — the balancer
/// cannot push unbounded throwaway conntrack inserts.
#[test]
fn ct_insert_gate_blocks_burst_above_cap() {
    let cap = 100_000u32;
    let gate = CtInsertGate::new(cap);
    let mut admitted = 0u64;
    let mut denied = 0u64;
    // Spray 3x the cap as fast as possible (single tight loop ≈ a
    // few ms wall-clock, so refill is negligible vs. the burst).
    for _ in 0..(cap as u64 * 3) {
        if gate.try_admit() {
            admitted += 1;
        } else {
            denied += 1;
        }
    }
    // The bucket starts full (== cap) and refills only by the tiny
    // wall-clock elapsed during the loop. Admitted must be bounded
    // well below the 3x spray, and a large fraction must be denied —
    // that denied fraction is what protects the LRU.
    assert!(
        admitted <= u64::from(cap) + u64::from(cap) / 10,
        "gate admitted {admitted} which exceeds cap {cap} + 10% refill slack",
    );
    assert!(
        denied >= u64::from(cap),
        "expected at least {cap} denials under a 3x burst, got {denied}",
    );
}

/// After the bucket drains, sleeping long enough to refill `n`
/// tokens lets `n` more inserts through — the window-boundary reset
/// the Katran lesson relies on (established flows survive across
/// windows; only the *new*-flow rate is capped per window).
#[test]
fn ct_insert_gate_refills_after_window() {
    let cap = 10_000u32;
    let gate = CtInsertGate::new(cap);
    // Drain the initial burst.
    while gate.try_admit() {}
    assert!(!gate.try_admit(), "bucket must be empty after draining");

    // Sleep 200 ms → expect ≈ cap * 0.2 = 2000 tokens refilled.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let mut refilled = 0u64;
    while gate.try_admit() {
        refilled += 1;
        if refilled > u64::from(cap) {
            break; // safety: never spin forever
        }
    }
    // Generous bounds: timer granularity + scheduler jitter. The
    // load-bearing assertion is "non-zero refill happened and it is
    // proportional, not the full burst".
    assert!(
        refilled >= 500,
        "expected ≥500 tokens refilled after 200ms at {cap}/s, got {refilled}",
    );
    assert!(
        refilled <= u64::from(cap),
        "refill {refilled} must not exceed the burst ceiling {cap}",
    );
}

/// `cap == 0` is the operator opt-out (config
/// `xdp_new_flow_cap_per_sec_per_cpu = 0`): every insert is admitted,
/// the gate is a no-op. Mirrors the eBPF-side `cap == 0 => return
/// false` branch in `is_under_flood()`.
#[test]
fn ct_insert_gate_zero_cap_disables() {
    let gate = CtInsertGate::new(0);
    for _ in 0..1_000_000 {
        assert!(gate.try_admit(), "cap=0 must admit unconditionally");
    }
}

/// The userspace default tracks Katran's per-core `MAX_CONN_RATE`
/// and the eBPF-side fallback constant — a single source of truth so
/// the data-plane and control-plane caps cannot silently diverge.
#[test]
fn default_cap_matches_katran_max_conn_rate() {
    assert_eq!(DEFAULT_NEW_FLOW_CAP_PER_SEC_PER_CPU, 125_000);
}
