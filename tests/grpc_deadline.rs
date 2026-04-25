use lb_grpc::*;

#[test]
fn test_grpc_deadline_propagation() {
    // Parse various timeout formats
    assert_eq!(GrpcDeadline::parse_timeout("5S").unwrap(), 5000);
    assert_eq!(GrpcDeadline::parse_timeout("100m").unwrap(), 100);
    assert_eq!(GrpcDeadline::parse_timeout("1H").unwrap(), 3_600_000);
    assert_eq!(GrpcDeadline::parse_timeout("2M").unwrap(), 120_000);

    // Test remaining calculation
    assert_eq!(GrpcDeadline::remaining(5000, 3000), Some(2000));
    assert_eq!(GrpcDeadline::remaining(5000, 6000), None);

    // Roundtrip
    let formatted = GrpcDeadline::format_timeout(5000);
    let parsed = GrpcDeadline::parse_timeout(&formatted).unwrap();
    assert_eq!(parsed, 5000);
}
