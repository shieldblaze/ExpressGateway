//! Session affinity (sticky sessions) load balancer integration tests.

use lb_balancer::session_affinity::SessionAffinity;
use lb_balancer::{Backend, KeyedLoadBalancer};

#[test]
fn test_balancer_session_affinity_sticky() {
    let lb = SessionAffinity::new();
    let backends: Vec<Backend> = (0..5).map(|i| Backend::new(format!("b{i}"), 1)).collect();

    // Same session key should always go to the same backend across 100 picks.
    for session_key in [42u64, 100, 999, 0, u64::MAX, 12345678] {
        let first = lb.pick_with_key(&backends, session_key).unwrap();
        for _ in 0..100 {
            let idx = lb.pick_with_key(&backends, session_key).unwrap();
            assert_eq!(
                idx, first,
                "session key {session_key} should always route to backend {first}, got {idx}"
            );
        }
    }
}

#[test]
fn test_balancer_session_affinity_different_sessions_spread() {
    let lb = SessionAffinity::new();
    let backends: Vec<Backend> = (0..4).map(|i| Backend::new(format!("b{i}"), 1)).collect();

    let mut counts = [0u32; 4];
    for key in 0..10_000u64 {
        let idx = lb.pick_with_key(&backends, key).unwrap();
        counts[idx] += 1;
    }

    // Different sessions should spread across backends.
    for (i, count) in counts.iter().enumerate() {
        assert!(
            *count > 1500 && *count < 3500,
            "backend {i} got {count}, expected reasonable spread around 2500"
        );
    }
}

#[test]
fn test_balancer_session_affinity_empty() {
    let lb = SessionAffinity::new();
    assert!(lb.pick_with_key(&[], 42).is_err());
}
