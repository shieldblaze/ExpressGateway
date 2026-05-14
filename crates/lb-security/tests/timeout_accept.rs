//! Proof for the TLS-handshake timeout helper (SEC-2-10).
//!
//! Wave-2c wires the call site in `crates/lb/src/main.rs`; this
//! test exercises the helper directly through the public API.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use lb_security::{DEFAULT_HANDSHAKE_TIMEOUT_MS, HandshakeError, timeout_accept};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::TlsAcceptor;

/// `AsyncRead` that always returns `Pending`. Drives the rustls
/// state machine into a permanent park.
#[derive(Debug)]
struct SilentStream;

impl AsyncRead for SilentStream {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

impl AsyncWrite for SilentStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn test_acceptor() -> TlsAcceptor {
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_der: Vec<u8> = generated.cert.der().to_vec();
    let key_der: Vec<u8> = generated.key_pair.serialize_der();
    let cert_chain = vec![rustls_pki_types::CertificateDer::from(cert_der)];
    let key =
        rustls_pki_types::PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(key_der));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .unwrap();
    TlsAcceptor::from(Arc::new(cfg))
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_slow_handshake_times_out() {
    // Plan-named headline test. A silent (no-byte) stream drives the
    // rustls state machine into a permanent park; the helper's
    // timeout must surface as HandshakeError::Timeout.
    let acceptor = test_acceptor();
    let err = timeout_accept(&acceptor, SilentStream, Duration::from_millis(500))
        .await
        .unwrap_err();
    match err {
        HandshakeError::Timeout { budget_ms } => assert_eq!(budget_ms, 500),
        other => panic!("expected Timeout, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn timeout_uses_default_budget_constant() {
    // The default 5 s budget is exposed via the public constant so
    // the Wave-2c call site can name it instead of hardcoding 5000.
    assert_eq!(DEFAULT_HANDSHAKE_TIMEOUT_MS, 5_000);
    let acceptor = test_acceptor();
    let err = timeout_accept(
        &acceptor,
        SilentStream,
        Duration::from_millis(DEFAULT_HANDSHAKE_TIMEOUT_MS),
    )
    .await
    .unwrap_err();
    match err {
        HandshakeError::Timeout { budget_ms } => assert_eq!(budget_ms, 5_000),
        other => panic!("expected Timeout, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn very_short_budget_still_returns_timeout() {
    let acceptor = test_acceptor();
    let err = timeout_accept(&acceptor, SilentStream, Duration::from_millis(1))
        .await
        .unwrap_err();
    assert!(matches!(err, HandshakeError::Timeout { .. }));
}
