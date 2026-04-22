//! HTTP/2 CONTINUATION flood integration test.
//!
//! Exercises the `ContinuationFloodDetector` from `lb-h2` to verify that
//! excessive CONTINUATION frames without END_HEADERS trigger detection.

use lb_h2::ContinuationFloodDetector;

#[test]
fn test_continuation_flood_detection() {
    let mut detector = ContinuationFloodDetector::new(5);
    // 5 CONTINUATION frames without END_HEADERS -- at limit, still ok
    for _ in 0..5 {
        assert!(detector.record(false).is_ok());
    }
    // The 6th exceeds the limit
    assert!(detector.record(false).is_err());
}
