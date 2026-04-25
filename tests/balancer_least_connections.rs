//! Least-connections load balancer integration tests.

use lb_balancer::least_connections::LeastConnections;
use lb_balancer::{Backend, LoadBalancer};

#[test]
fn test_balancer_least_conn_picks_min() {
    let mut lb = LeastConnections::new();
    let mut backends = vec![
        Backend::new("a", 1),
        Backend::new("b", 1),
        Backend::new("c", 1),
    ];
    backends[0].active_connections = 10;
    backends[1].active_connections = 5;
    backends[2].active_connections = 3;

    let idx = lb.pick(&backends).unwrap();
    assert_eq!(
        idx, 2,
        "should pick backend with fewest connections (index 2)"
    );
}

#[test]
fn test_balancer_least_conn_tie_breaks_by_index() {
    let mut lb = LeastConnections::new();
    let mut backends = vec![
        Backend::new("a", 1),
        Backend::new("b", 1),
        Backend::new("c", 1),
    ];
    backends[0].active_connections = 5;
    backends[1].active_connections = 5;
    backends[2].active_connections = 5;

    let idx = lb.pick(&backends).unwrap();
    assert_eq!(idx, 0, "ties should be broken by lowest index");
}

#[test]
fn test_balancer_least_conn_empty() {
    let mut lb = LeastConnections::new();
    assert!(lb.pick(&[]).is_err());
}
