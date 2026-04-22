use bytes::Bytes;
use lb_grpc::*;

#[test]
fn test_grpc_server_streaming() {
    // Encode multiple frames back-to-back (simulating server stream)
    let frames: Vec<GrpcFrame> = (0..5)
        .map(|i| GrpcFrame {
            compressed: false,
            data: Bytes::from(format!("message {i}")),
        })
        .collect();

    let mut buf = Vec::new();
    for f in &frames {
        buf.extend_from_slice(&encode_grpc_frame(f).unwrap());
    }

    // Decode all frames sequentially
    let mut offset = 0;
    for (i, _) in frames.iter().enumerate() {
        let (decoded, consumed) =
            decode_grpc_frame(&buf[offset..], DEFAULT_MAX_MESSAGE_SIZE).unwrap();
        assert_eq!(decoded.data, Bytes::from(format!("message {i}")));
        offset += consumed;
    }
    assert_eq!(offset, buf.len());
}
