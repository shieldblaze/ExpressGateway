use bytes::Bytes;
use lb_grpc::*;

#[test]
fn test_grpc_unary_roundtrip() {
    let frame = GrpcFrame {
        compressed: false,
        data: Bytes::from("hello grpc"),
    };
    let encoded = encode_grpc_frame(&frame).unwrap();
    let (decoded, consumed) = decode_grpc_frame(&encoded, DEFAULT_MAX_MESSAGE_SIZE).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded.data, frame.data);
    assert!(!decoded.compressed);
}
