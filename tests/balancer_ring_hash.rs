//! Ring-hash consistent hashing load balancer integration tests.

use lb_balancer::ring_hash::RingHash;
use lb_balancer::{Backend, KeyedLoadBalancer};

fn make_backends(n: usize) -> Vec<Backend> {
    (0..n)
        .map(|i| Backend::new(format!("ring-backend-{i}"), 1))
        .collect()
}

#[test]
fn test_balancer_ring_hash_consistent() {
    let backends = make_backends(5);
    let ring = RingHash::new(&backends).unwrap();

    // Same key always returns the same backend.
    for key in 0..1000u64 {
        let first = ring.pick_with_key(&backends, key).unwrap();
        let second = ring.pick_with_key(&backends, key).unwrap();
        assert_eq!(first, second, "key {key} should map to the same backend");
    }
}

#[test]
fn test_balancer_ring_hash_minimal_disruption() {
    // Removing one backend should not affect mappings of keys that belong to other backends.
    let backends_5 = make_backends(5);
    let backends_4: Vec<Backend> = backends_5.iter().take(4).cloned().collect();

    let ring_5 = RingHash::new(&backends_5).unwrap();
    let ring_4 = RingHash::new(&backends_4).unwrap();

    let total = 10_000u64;
    let mut same_count = 0u64;
    let mut comparable = 0u64;

    for key in 0..total {
        let idx_5 = ring_5.pick_with_key(&backends_5, key).unwrap();
        let idx_4 = ring_4.pick_with_key(&backends_4, key).unwrap();
        // Only count keys that mapped to still-existing backends.
        if idx_5 < 4 {
            comparable += 1;
            if idx_5 == idx_4 {
                same_count += 1;
            }
        }
    }

    let pct = if comparable > 0 {
        (same_count as f64) / (comparable as f64) * 100.0
    } else {
        0.0
    };
    assert!(
        pct > 60.0,
        "too much disruption: only {pct:.1}% of keys to remaining backends stayed stable"
    );
}

#[test]
fn test_balancer_ring_hash_distribution() {
    let backends = make_backends(4);
    let ring = RingHash::new(&backends).unwrap();

    let mut counts = [0u32; 4];
    for key in 0..10_000u64 {
        let idx = ring.pick_with_key(&backends, key).unwrap();
        counts[idx] += 1;
    }

    // Each backend should get a non-trivial share.
    for (i, count) in counts.iter().enumerate() {
        assert!(
            *count > 500,
            "backend {i} only got {count} picks, expected > 500"
        );
    }
}
