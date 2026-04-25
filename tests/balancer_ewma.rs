//! EWMA (latency-based) load balancer integration tests.

use lb_balancer::ewma::Ewma;
use lb_balancer::{Backend, LoadBalancer};

#[test]
fn test_balancer_ewma_prefers_fast() {
    let mut lb = Ewma::new();
    let mut backends = vec![Backend::new("fast", 1), Backend::new("slow", 1)];
    backends[0].latency_ewma_ns = 10_000_000; // 10ms
    backends[1].latency_ewma_ns = 100_000_000; // 100ms

    // With equal connections, the fast backend should always win.
    for _ in 0..100 {
        let idx = lb.pick(&backends).unwrap();
        assert_eq!(idx, 0, "EWMA should overwhelmingly pick the fast backend");
    }
}

#[test]
fn test_balancer_ewma_load_factor() {
    let mut lb = Ewma::new();
    let mut backends = vec![Backend::new("fast-loaded", 1), Backend::new("slow-idle", 1)];
    // Fast but overloaded: score = 1ms * (100+1) = 101_000_000
    backends[0].latency_ewma_ns = 1_000_000;
    backends[0].active_connections = 100;
    // Slow but idle: score = 50ms * (0+1) = 50_000_000
    backends[1].latency_ewma_ns = 50_000_000;
    backends[1].active_connections = 0;

    let idx = lb.pick(&backends).unwrap();
    assert_eq!(
        idx, 1,
        "idle slow backend should beat overloaded fast backend"
    );
}

#[test]
fn test_balancer_ewma_zero_latency() {
    let mut lb = Ewma::new();
    let mut backends = vec![Backend::new("zero", 1), Backend::new("nonzero", 1)];
    backends[0].latency_ewma_ns = 0;
    backends[1].latency_ewma_ns = 1;

    let idx = lb.pick(&backends).unwrap();
    assert_eq!(idx, 0, "zero-latency backend should be preferred");
}

#[test]
fn test_balancer_ewma_empty() {
    let mut lb = Ewma::new();
    assert!(lb.pick(&[]).is_err());
}
