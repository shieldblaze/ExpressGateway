//! Native QUIC forwarding tests.
//!
//! Pillar 3a replaces the former userspace simulation with a real quinn 0.11
//! endpoint bound to 127.0.0.1. Each test spins up a server and a client
//! endpoint, completes a real UDP + TLS 1.3 handshake against a self-signed
//! certificate generated in-process by rcgen, and asserts that the bytes
//! the client sent are the bytes the server received.
//!
//! The two test function names are manifest-locked
//! (`manifest/required-tests.txt`) and must not be renamed.

use std::sync::Arc;

use lb_quic::{QuicDatagram, QuicEndpoint, QuicStream, roundtrip_datagram, roundtrip_stream};
use rustls::RootCertStore;
use rustls_pki_types::CertificateDer;

fn generate_loopback_cert() -> (Vec<u8>, Vec<u8>, CertificateDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let cert_der: Vec<u8> = generated.cert.der().to_vec();
    let key_der: Vec<u8> = generated.key_pair.serialize_der();
    let trust_anchor = CertificateDer::from(cert_der.clone());
    (cert_der, key_der, trust_anchor)
}

fn build_client_roots(trust_anchor: CertificateDer<'static>) -> Arc<RootCertStore> {
    let mut roots = RootCertStore::empty();
    roots.add(trust_anchor).unwrap();
    Arc::new(roots)
}

#[tokio::test]
async fn test_quic_datagram_forwarding() {
    let (cert_der, key_der, trust_anchor) = generate_loopback_cert();
    let server = QuicEndpoint::server_on_loopback(cert_der, key_der).unwrap();
    let roots = build_client_roots(trust_anchor);
    let client = QuicEndpoint::client_on_loopback(roots).unwrap();

    // Sanity: server is bound to a loopback UDP port, not the simulated
    // in-process channel the previous implementation used.
    assert!(server.local_addr().ip().is_loopback());
    assert_ne!(server.local_addr().port(), 0);

    let dg = QuicDatagram {
        connection_id: 42,
        data: b"hello".to_vec(),
    };
    let forwarded = roundtrip_datagram(&server, &client, dg).await.unwrap();
    assert_eq!(forwarded.connection_id, 42);
    assert_eq!(forwarded.data, b"hello");
}

#[tokio::test]
async fn test_quic_stream_forwarding() {
    let (cert_der, key_der, trust_anchor) = generate_loopback_cert();
    let server = QuicEndpoint::server_on_loopback(cert_der, key_der).unwrap();
    let roots = build_client_roots(trust_anchor);
    let client = QuicEndpoint::client_on_loopback(roots).unwrap();

    assert!(server.local_addr().ip().is_loopback());

    let stream = QuicStream {
        stream_id: 1,
        data: b"stream data".to_vec(),
        fin: true,
    };
    let forwarded = roundtrip_stream(&server, &client, stream).await.unwrap();
    assert_eq!(forwarded.stream_id, 1);
    assert_eq!(forwarded.data, b"stream data");
    assert!(forwarded.fin);
}
