//! Native QUIC forwarding tests.
//!
//! Pillar 3b.1 put this crate on real quiche 0.28 + BoringSSL; Pillar
//! 3b.3a turns client peer-cert verification back on. The loopback
//! cert is now built as a CA (basicConstraints=CA:TRUE) with a DNS
//! SAN of `expressgateway.test` and a `serverAuth` EKU so BoringSSL's
//! hostname verifier accepts it on the client side. The client still
//! connects to `SocketAddr(127.0.0.1, <port>)`; only the TLS SNI
//! uses the hostname.
//!
//! Each test writes the self-signed cert + key + CA-bundle PEM triple
//! to a temp dir, spins up a server and a client [`QuicEndpoint`] on
//! `127.0.0.1:0`, completes a real UDP + TLS 1.3 handshake with peer
//! verification active, and asserts roundtrip preservation.
//!
//! The two test function names `test_quic_datagram_forwarding` and
//! `test_quic_stream_forwarding` are manifest-locked; other test names
//! in this file are not.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lb_quic::{
    QuicDatagram, QuicEndpoint, QuicStream, ZeroRttReplayGuard, roundtrip_datagram,
    roundtrip_stream,
};
use parking_lot::Mutex;

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
    // basicConstraints=CA:TRUE AND a matching hostname in the SAN. An
    // iPAddress-type SAN with `127.0.0.1` + `verify_peer(true)` was
    // verified empirically to still fail BoringSSL's hostname check
    // (TlsFail) even with a `serverAuth` EKU, so Pillar 3b.3a takes
    // the task's documented fallback: mint the cert with a DNS SAN of
    // `expressgateway.test` and have the client connect with that
    // hostname as SNI while still targeting `SocketAddr(127.0.0.1,
    // <port>)`. The TCP/UDP endpoint stays loopback; only the TLS
    // peer-name-matching surface uses a hostname.
    let mut params =
        rcgen::CertificateParams::new(vec!["expressgateway.test".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
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

#[tokio::test]
async fn replay_filter_rejects_second_0rtt_with_same_token() {
    // Pillar 3b.3a exposes the 0-RTT replay seam on QuicEndpoint. The
    // quiche wire path does not yet feed this filter per-Initial
    // (custom accept loop lands in Pillar 3b.3c), so this test
    // exercises the observable contract directly: installing a filter
    // on the endpoint makes it reachable via `replay_filter()`, and
    // the filter detects a repeated 0-RTT token.
    let certs = generate_loopback_certs();
    let filter = Arc::new(Mutex::new(ZeroRttReplayGuard::new(16)));
    let server = QuicEndpoint::server_on_loopback(certs.cert.clone(), certs.key.clone())
        .await
        .unwrap()
        .with_replay_filter(Arc::clone(&filter));

    let installed = server.replay_filter().expect("replay filter is installed");
    let session_token = b"session-ticket-id-abcdef012345";
    assert!(installed.lock().check_0rtt_token(session_token).is_ok());
    assert!(
        installed.lock().check_0rtt_token(session_token).is_err(),
        "second use of the same 0-RTT token must be rejected"
    );
    // A fresh token is unaffected.
    assert!(
        installed
            .lock()
            .check_0rtt_token(b"different-token")
            .is_ok()
    );
}
