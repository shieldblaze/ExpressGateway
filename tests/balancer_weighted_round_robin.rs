//! Weighted round-robin load balancer integration tests.

use lb_balancer::weighted_round_robin::WeightedRoundRobin;
use lb_balancer::{Backend, LoadBalancer};

#[test]
fn test_balancer_wrr_respects_weights() {
    let mut wrr = WeightedRoundRobin::new();
    let backends = vec![
        Backend::new("a", 5),
        Backend::new("b", 3),
        Backend::new("c", 2),
    ];

    let mut counts = [0u32; 3];
    // 100 picks with total weight 10 => exactly 50:30:20
    for _ in 0..100 {
        let idx = wrr.pick(&backends).unwrap();
        counts[idx] += 1;
    }

    assert_eq!(
        counts[0], 50,
        "weight-5 backend should get 50, got {}",
        counts[0]
    );
    assert_eq!(
        counts[1], 30,
        "weight-3 backend should get 30, got {}",
        counts[1]
    );
    assert_eq!(
        counts[2], 20,
        "weight-2 backend should get 20, got {}",
        counts[2]
    );
}

#[test]
fn test_balancer_wrr_smooth_interleaving() {
    // Verify the Nginx-style smooth WRR does not produce bursts.
    let mut wrr = WeightedRoundRobin::new();
    let backends = vec![Backend::new("a", 5), Backend::new("b", 1)];

    // Over 6 picks (total weight = 6), backend "a" should appear 5 times
    // and backend "b" should appear once, but NOT as [a,a,a,a,a,b].
    let mut sequence = Vec::new();
    for _ in 0..6 {
        sequence.push(wrr.pick(&backends).unwrap());
    }

    // "b" (index 1) should not be at the very end (that would be non-smooth).
    let b_positions: Vec<usize> = sequence
        .iter()
        .enumerate()
        .filter(|&(_, idx)| *idx == 1)
        .map(|(pos, _)| pos)
        .collect();
    assert_eq!(b_positions.len(), 1);
    // b should not be at position 5 (last), proving smooth interleaving.
    assert_ne!(
        b_positions[0], 5,
        "smooth WRR should not cluster all a's before b"
    );
}

#[test]
fn test_balancer_wrr_all_zero_weight() {
    let mut wrr = WeightedRoundRobin::new();
    let backends = vec![Backend::new("a", 0), Backend::new("b", 0)];
    assert!(wrr.pick(&backends).is_err());
}
