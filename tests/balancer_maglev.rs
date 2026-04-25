//! Maglev consistent hashing load balancer integration tests.

use lb_balancer::maglev::Maglev;
use lb_balancer::{Backend, KeyedLoadBalancer};

fn make_backends(n: usize) -> Vec<Backend> {
    (0..n)
        .map(|i| Backend::new(format!("backend-{i}"), 1))
        .collect()
}

#[test]
fn test_balancer_maglev_consistent() {
    let backends = make_backends(5);
    let maglev = Maglev::new(&backends).unwrap();

    // Same key always maps to the same backend.
    for key in 0..1000u64 {
        let first = maglev.pick_with_key(&backends, key).unwrap();
        let second = maglev.pick_with_key(&backends, key).unwrap();
        assert_eq!(
            first, second,
            "key {key} should always map to the same backend"
        );
    }
}

#[test]
fn test_balancer_maglev_minimal_disruption() {
    // Adding a backend should only remap ~1/N keys.
    let backends_5 = make_backends(5);
    let backends_6 = make_backends(6);

    let maglev_5 = Maglev::new(&backends_5).unwrap();
    let maglev_6 = Maglev::new(&backends_6).unwrap();

    let total = 10_000u64;
    let mut same_count = 0u64;
    let mut comparable = 0u64;

    for key in 0..total {
        let idx_5 = maglev_5.pick_with_key(&backends_5, key).unwrap();
        let idx_6 = maglev_6.pick_with_key(&backends_6, key).unwrap();
        // Only compare keys that mapped to backends 0-4 in both tables.
        if idx_5 < 5 && idx_6 < 5 {
            comparable += 1;
            if idx_5 == idx_6 {
                same_count += 1;
            }
        }
    }

    // Most keys that stay within the original 5 backends should be stable.
    let pct = if comparable > 0 {
        (same_count as f64) / (comparable as f64) * 100.0
    } else {
        0.0
    };
    assert!(
        pct > 50.0,
        "too much disruption: only {pct:.1}% of comparable keys stayed stable"
    );
}

#[test]
fn test_balancer_maglev_distribution() {
    // Verify all backends get some traffic.
    let backends = make_backends(5);
    let maglev = Maglev::new(&backends).unwrap();

    let mut counts = [0u32; 5];
    for key in 0..10_000u64 {
        let idx = maglev.pick_with_key(&backends, key).unwrap();
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
