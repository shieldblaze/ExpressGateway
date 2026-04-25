use lb_grpc::*;

#[test]
fn test_grpc_status_translation() {
    // gRPC status -> HTTP status
    assert_eq!(GrpcStatus::Ok.to_http_status(), 200);
    assert_eq!(GrpcStatus::NotFound.to_http_status(), 404);
    assert_eq!(GrpcStatus::Unauthenticated.to_http_status(), 401);
    assert_eq!(GrpcStatus::PermissionDenied.to_http_status(), 403);
    assert_eq!(GrpcStatus::Unavailable.to_http_status(), 503);

    // HTTP status -> gRPC status
    assert_eq!(GrpcStatus::from_http_status(200), GrpcStatus::Ok);
    assert_eq!(
        GrpcStatus::from_http_status(401),
        GrpcStatus::Unauthenticated
    );
    assert_eq!(GrpcStatus::from_http_status(503), GrpcStatus::Unavailable);

    // From code
    assert_eq!(GrpcStatus::from_code(0).unwrap(), GrpcStatus::Ok);
    assert!(GrpcStatus::from_code(99).is_err());
}
