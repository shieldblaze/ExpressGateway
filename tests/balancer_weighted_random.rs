//! Weighted random load balancer integration tests.

use lb_balancer::weighted_random::WeightedRandom;
use lb_balancer::{Backend, LoadBalancer};
use rand::SeedableRng;
use rand::rngs::StdRng;

#[test]
fn test_balancer_wrandom_respects_weights() {
    let rng = StdRng::seed_from_u64(42);
    let mut lb = WeightedRandom::new(rng);
    let backends = vec![
        Backend::new("a", 7),
        Backend::new("b", 2),
        Backend::new("c", 1),
    ];

    let mut counts = [0u32; 3];
    for _ in 0..10_000 {
        let idx = lb.pick(&backends).unwrap();
        counts[idx] += 1;
    }

    // Expected: ~7000, ~2000, ~1000 with total weight = 10.
    // Allow generous bounds for randomness.
    assert!(
        counts[0] >= 6000 && counts[0] <= 8000,
        "weight-7 backend got {}, expected 6000-8000",
        counts[0]
    );
    assert!(
        counts[1] >= 1200 && counts[1] <= 2800,
        "weight-2 backend got {}, expected 1200-2800",
        counts[1]
    );
    assert!(
        counts[2] >= 400 && counts[2] <= 1600,
        "weight-1 backend got {}, expected 400-1600",
        counts[2]
    );
}

#[test]
fn test_balancer_wrandom_all_zero_weight() {
    let rng = StdRng::seed_from_u64(0);
    let mut lb = WeightedRandom::new(rng);
    let backends = vec![Backend::new("a", 0), Backend::new("b", 0)];
    assert!(lb.pick(&backends).is_err());
}
