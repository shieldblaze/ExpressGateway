use bytes::Bytes;
use lb_grpc::*;

#[test]
fn test_grpc_client_streaming() {
    // Same as server streaming but conceptually from client
    let frame = GrpcFrame {
        compressed: false,
        data: Bytes::from(vec![42u8; 100]),
    };
    let encoded = encode_grpc_frame(&frame).unwrap();
    let (decoded, _) = decode_grpc_frame(&encoded, DEFAULT_MAX_MESSAGE_SIZE).unwrap();
    assert_eq!(decoded.data.len(), 100);
}
