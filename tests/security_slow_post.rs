use lb_security::*;

#[test]
fn test_slow_post_rejected() {
    let mut detector = SlowPostDetector::new(10000, 100); // 10s timeout, 100 B/s min

    // 50 bytes in 5 seconds = 10 B/s, well below 100 B/s threshold
    let result = detector.record_body_bytes(50, 5000, 10000);
    assert!(result.is_err());
}
