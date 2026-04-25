use lb_security::*;

#[test]
fn test_zero_rtt_replay_rejected() {
    let mut guard = ZeroRttReplayGuard::new(1000);
    let token = b"unique-token-abc123";

    // First use -> OK
    assert!(guard.check_and_record(token).is_ok());

    // Second use -> replay detected
    assert!(guard.check_and_record(token).is_err());

    // Different token -> OK
    assert!(guard.check_and_record(b"different-token").is_ok());
}
