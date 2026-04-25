//! Power-of-two-choices (P2C) load balancer integration tests.

use lb_balancer::p2c::PowerOfTwoChoices;
use lb_balancer::{Backend, LoadBalancer};
use rand::SeedableRng;
use rand::rngs::StdRng;

#[test]
fn test_balancer_p2c_converges() {
    let rng = StdRng::seed_from_u64(42);
    let mut lb = PowerOfTwoChoices::new(rng);

    let mut backends: Vec<Backend> = (0..4).map(|i| Backend::new(format!("b{i}"), 1)).collect();
    // Heavily loaded
    backends[0].active_connections = 100;
    backends[1].active_connections = 100;
    // Lightly loaded
    backends[2].active_connections = 1;
    backends[3].active_connections = 1;

    let mut counts = [0u32; 4];
    for _ in 0..1_000 {
        let idx = lb.pick(&backends).unwrap();
        counts[idx] += 1;
    }

    // The lightly loaded backends should get the vast majority of picks.
    let heavy = counts[0] + counts[1];
    let light = counts[2] + counts[3];
    assert!(
        light > heavy,
        "light backends got {light}, heavy got {heavy} -- P2C should prefer light"
    );
    assert!(
        light > 700,
        "light backends only got {light}/1000, expected > 700"
    );
}

#[test]
fn test_balancer_p2c_single_backend() {
    let rng = StdRng::seed_from_u64(0);
    let mut lb = PowerOfTwoChoices::new(rng);
    let backends = vec![Backend::new("only", 1)];
    assert_eq!(lb.pick(&backends).unwrap(), 0);
}

#[test]
fn test_balancer_p2c_empty() {
    let rng = StdRng::seed_from_u64(0);
    let mut lb = PowerOfTwoChoices::new(rng);
    assert!(lb.pick(&[]).is_err());
}
