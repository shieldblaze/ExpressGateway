//! Random load balancer integration tests.

use lb_balancer::random::Random;
use lb_balancer::{Backend, LoadBalancer};
use rand::SeedableRng;
use rand::rngs::StdRng;

#[test]
fn test_balancer_random_uniform() {
    let rng = StdRng::seed_from_u64(12345);
    let mut lb = Random::new(rng);
    let backends: Vec<Backend> = (0..4).map(|i| Backend::new(format!("b{i}"), 1)).collect();

    let mut counts = [0u32; 4];
    for _ in 0..10_000 {
        let idx = lb.pick(&backends).unwrap();
        counts[idx] += 1;
    }

    // With 10000 picks over 4 backends, each should get ~2500.
    // Allow 2000-3000 for statistical safety.
    for (i, count) in counts.iter().enumerate() {
        assert!(
            *count >= 2000 && *count <= 3000,
            "backend {i} got {count} picks, expected 2000-3000"
        );
    }
}

#[test]
fn test_balancer_random_empty() {
    let rng = StdRng::seed_from_u64(0);
    let mut lb = Random::new(rng);
    assert!(lb.pick(&[]).is_err());
}
