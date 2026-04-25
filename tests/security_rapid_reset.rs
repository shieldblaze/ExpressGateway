//! HTTP/2 rapid reset (CVE-2023-44487) integration test.
//!
//! Exercises the `RapidResetDetector` from `lb-h2` to verify that excessive
//! RST_STREAM frames within a window trigger detection.

use lb_h2::RapidResetDetector;

#[test]
fn test_rapid_reset_detection() {
    let mut detector = RapidResetDetector::new(5, 1000);
    // Feed 5 RST_STREAM events (at threshold -- still ok)
    for tick in 0..5 {
        assert!(detector.record(tick).is_ok());
    }
    // The 6th exceeds the threshold
    assert!(detector.record(5).is_err());
}
