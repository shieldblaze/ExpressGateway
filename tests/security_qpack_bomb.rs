//! QPACK bomb integration test.
//!
//! Exercises the `QpackBombDetector` from `lb-h3` to verify that excessive
//! decompression ratios are detected and rejected.

use lb_h3::QpackBombDetector;

#[test]
fn test_qpack_bomb_detection() {
    let detector = QpackBombDetector::new(100, 65536);
    // Normal ratio: 2:1 -- ok
    assert!(detector.check(1000, 2000).is_ok());
    // Bomb ratio: 200:1 with large decoded size -- rejected
    assert!(detector.check(1024, 204_800).is_err());
    // Absolute size exceeded
    assert!(detector.check(10_000, 100_000).is_err());
}
