//! Least-request load balancer integration tests.

use lb_balancer::least_request::LeastRequest;
use lb_balancer::{Backend, LoadBalancer};

#[test]
fn test_balancer_least_req_picks_min() {
    let mut lb = LeastRequest::new();
    let mut backends = vec![
        Backend::new("a", 1),
        Backend::new("b", 1),
        Backend::new("c", 1),
    ];
    backends[0].active_requests = 10;
    backends[1].active_requests = 2;
    backends[2].active_requests = 8;

    let idx = lb.pick(&backends).unwrap();
    assert_eq!(idx, 1, "should pick backend with fewest requests (index 1)");
}

#[test]
fn test_balancer_least_req_tie_breaks_by_index() {
    let mut lb = LeastRequest::new();
    let backends = vec![Backend::new("a", 1), Backend::new("b", 1)];
    // Both have 0 requests -- index 0 wins.
    assert_eq!(lb.pick(&backends).unwrap(), 0);
}

#[test]
fn test_balancer_least_req_empty() {
    let mut lb = LeastRequest::new();
    assert!(lb.pick(&[]).is_err());
}
