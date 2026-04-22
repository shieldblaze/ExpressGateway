//! BREACH attack posture tests.

use lb_compression::*;

#[test]
fn test_compression_breach_posture_no_leak() {
    // When both secret headers and reflected input are present, compression is blocked
    assert!(!BreachGuard::should_compress(true, true));
    // When only one condition is true, compression is allowed
    assert!(BreachGuard::should_compress(true, false));
    assert!(BreachGuard::should_compress(false, true));
    assert!(BreachGuard::should_compress(false, false));
}
