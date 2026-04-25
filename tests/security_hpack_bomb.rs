//! HPACK bomb integration test.
//!
//! Exercises the `HpackBombDetector` from `lb-h2` to verify that excessive
//! decompression ratios are detected and rejected.

use lb_h2::HpackBombDetector;

#[test]
fn test_hpack_bomb_detection() {
    let detector = HpackBombDetector::new(100, 65536);
    // Normal ratio: 2:1 -- ok
    assert!(detector.check(1000, 2000).is_ok());
    // Bomb ratio: 200:1 with large decoded size -- rejected
    assert!(detector.check(1024, 204_800).is_err());
    // Absolute size exceeded
    assert!(detector.check(10_000, 100_000).is_err());
}
