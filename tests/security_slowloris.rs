use lb_security::*;

#[test]
fn test_slowloris_connection_reaped() {
    let detector = SlowlorisDetector::new(5000, 100); // 5s timeout, 100 B/s min

    // Simulate slow drip: 10 bytes over 6 seconds
    assert!(detector.check_header_timeout(6000).is_err());

    // Fast connection: check at 1 second -> OK
    let fast = SlowlorisDetector::new(5000, 100);
    assert!(fast.check_header_timeout(1000).is_ok());
}
