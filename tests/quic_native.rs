//! Native QUIC forwarding tests.
//!
//! Pillar 3b.1 runs on quiche 0.28 + BoringSSL. Each test writes a
//! self-signed rcgen cert+key to a temp dir, spins up a server and a
//! client [`QuicEndpoint`] on `127.0.0.1:0`, completes a real UDP + TLS
//! 1.3 handshake, and asserts roundtrip preservation.
//!
//! The two test function names are manifest-locked
//! (`manifest/required-tests.txt`) and must not be renamed.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use lb_quic::{QuicDatagram, QuicEndpoint, QuicStream, roundtrip_datagram, roundtrip_stream};

static CERT_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestCerts {
    _dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
}

impl Drop for TestCerts {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self._dir);
    }
}

fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = CERT_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "lb-quic-test-{}-{}-{counter}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();

    // BoringSSL's verifier requires the trust anchor to carry
    // basicConstraints=CA:TRUE. rcgen's `generate_simple_self_signed`
    // default is a leaf cert (IsCa::NoCa), which BoringSSL rejects as a
    // root. Build the params explicitly so the same self-signed cert
    // can serve as both the server leaf and the client's CA anchor.
    let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    fs::write(&cert_path, cert_pem.as_bytes()).unwrap();
    fs::write(&key_path, key_pem.as_bytes()).unwrap();
    // For a self-signed loopback cert the server cert doubles as the CA.
    fs::write(&ca_path, cert_pem.as_bytes()).unwrap();
    TestCerts {
        _dir: dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
    }
}

#[tokio::test]
async fn test_quic_datagram_forwarding() {
    let certs = generate_loopback_certs();
    let server = QuicEndpoint::server_on_loopback(certs.cert.clone(), certs.key.clone())
        .await
        .unwrap();
    let client = QuicEndpoint::client_on_loopback(certs.ca.clone())
        .await
        .unwrap();

    // Sanity: both endpoints bound to loopback UDP ports.
    assert!(server.local_addr().ip().is_loopback());
    assert_ne!(server.local_addr().port(), 0);
    assert!(client.local_addr().ip().is_loopback());

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
    let certs = generate_loopback_certs();
    let server = QuicEndpoint::server_on_loopback(certs.cert.clone(), certs.key.clone())
        .await
        .unwrap();
    let client = QuicEndpoint::client_on_loopback(certs.ca.clone())
        .await
        .unwrap();

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
