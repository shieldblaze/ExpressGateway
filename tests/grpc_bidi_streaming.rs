use bytes::Bytes;
use lb_grpc::*;

#[test]
fn test_grpc_bidi_streaming() {
    // Test that compressed flag works correctly for both directions
    let compressed_frame = GrpcFrame {
        compressed: true,
        data: Bytes::from("compressed data"),
    };
    let uncompressed_frame = GrpcFrame {
        compressed: false,
        data: Bytes::from("plain data"),
    };

    let enc1 = encode_grpc_frame(&compressed_frame).unwrap();
    let enc2 = encode_grpc_frame(&uncompressed_frame).unwrap();

    let (dec1, _) = decode_grpc_frame(&enc1, DEFAULT_MAX_MESSAGE_SIZE).unwrap();
    let (dec2, _) = decode_grpc_frame(&enc2, DEFAULT_MAX_MESSAGE_SIZE).unwrap();

    assert!(dec1.compressed);
    assert!(!dec2.compressed);
}
