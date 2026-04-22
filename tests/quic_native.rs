//! Native QUIC forwarding tests.

use lb_quic::{QuicDatagram, QuicStream, forward_datagram, forward_stream};

#[test]
fn test_quic_datagram_forwarding() {
    let dg = QuicDatagram {
        connection_id: 42,
        data: b"hello".to_vec(),
    };
    let forwarded = forward_datagram(&dg).unwrap();
    assert_eq!(forwarded.connection_id, 42);
    assert_eq!(forwarded.data, b"hello");
}

#[test]
fn test_quic_stream_forwarding() {
    let stream = QuicStream {
        stream_id: 1,
        data: b"stream data".to_vec(),
        fin: true,
    };
    let forwarded = forward_stream(&stream).unwrap();
    assert_eq!(forwarded.stream_id, 1);
    assert!(forwarded.fin);
}
