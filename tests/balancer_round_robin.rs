//! Round-robin load balancer integration tests.

use lb_balancer::round_robin::RoundRobin;
use lb_balancer::{Backend, LoadBalancer};

#[test]
fn test_balancer_round_robin_distribution() {
    let mut rr = RoundRobin::new();
    let backends: Vec<Backend> = (0..3).map(|i| Backend::new(format!("b{i}"), 1)).collect();

    let mut counts = [0u32; 3];
    for _ in 0..300 {
        let idx = rr.pick(&backends).unwrap();
        counts[idx] += 1;
    }

    // Perfect round-robin: each gets exactly 100.
    assert_eq!(
        counts[0], 100,
        "backend 0 should get exactly 100, got {}",
        counts[0]
    );
    assert_eq!(
        counts[1], 100,
        "backend 1 should get exactly 100, got {}",
        counts[1]
    );
    assert_eq!(
        counts[2], 100,
        "backend 2 should get exactly 100, got {}",
        counts[2]
    );
}

#[test]
fn test_balancer_round_robin_empty() {
    let mut rr = RoundRobin::new();
    let result = rr.pick(&[]);
    assert!(result.is_err());
}

#[test]
fn test_balancer_round_robin_single_backend() {
    let mut rr = RoundRobin::new();
    let backends = vec![Backend::new("only", 1)];
    for _ in 0..10 {
        assert_eq!(rr.pick(&backends).unwrap(), 0);
    }
}
