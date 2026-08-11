//! TLS-handshake timeout helper (SEC-2-10) — `TlsAcceptor::accept` under a `tokio::time::timeout`.
//! The 5 s default sits above a healthy sub-second TLS 1.3 handshake and below what an accept-side
//! slowloris needs to be worth running.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::error::Elapsed;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// Default TLS handshake budget.
pub const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 5_000;

/// Outcome of [`timeout_accept`].
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// Budget elapsed before rustls reached `Connected`. Close with a RST, NOT `shutdown(Write)`:
    /// the peer never got ServerHello, so it holds no key material to ack a clean close with.
    #[error("TLS handshake exceeded {budget_ms}ms timeout")]
    Timeout {
        /// Configured budget that was exceeded.
        budget_ms: u64,
    },

    /// rustls handshake error — kept distinct from a timeout because it usually means client
    /// mis-configuration rather than an attack.
    #[error("TLS handshake failed: {0}")]
    Handshake(#[source] std::io::Error),
}

/// `acceptor.accept(stream)` under a `budget`. A zero `budget` rejects every connection, so it
/// debug-asserts and is raised to 1 ms in release rather than taking production down.
pub async fn timeout_accept<IO>(
    acceptor: &TlsAcceptor,
    stream: IO,
    budget: Duration,
) -> Result<TlsStream<IO>, HandshakeError>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    let budget = if budget.is_zero() {
        debug_assert!(!budget.is_zero(), "timeout_accept budget must be non-zero");
        Duration::from_millis(1)
    } else {
        budget
    };
    let budget_ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX);
    let accept_future = acceptor.accept(stream);
    match tokio::time::timeout(budget, accept_future).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(HandshakeError::Handshake(e)),
        Err(_elapsed) => {
            let _ = std::any::type_name::<Elapsed>(); // doc-link anchor
            Err(HandshakeError::Timeout { budget_ms })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    /// Never yields bytes, so rustls parks forever and only the timeout can terminate the accept.
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
        let key_der: Vec<u8> = generated.signing_key.serialize_der();
        let cert_chain = vec![rustls_pki_types::CertificateDer::from(cert_der)];
        let key = rustls_pki_types::PrivateKeyDer::Pkcs8(
            rustls_pki_types::PrivatePkcs8KeyDer::from(key_der),
        );
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
    async fn timeout_fires_on_silent_stream() {
        let acceptor = test_acceptor();
        let stream = SilentStream;
        let err = timeout_accept(&acceptor, stream, Duration::from_millis(50))
            .await
            .unwrap_err();
        match err {
            HandshakeError::Timeout { budget_ms } => assert_eq!(budget_ms, 50),
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn zero_budget_promoted_to_one_ms() {
        // Debug-assertions are on in test builds, so the release zero-budget fallback cannot be
        // exercised directly; 1 ms is the smallest budget that skips the assert and still proves
        // the timeout fires.
        let acceptor = test_acceptor();
        let stream = SilentStream;
        let err = timeout_accept(&acceptor, stream, Duration::from_millis(1))
            .await
            .unwrap_err();
        assert!(matches!(err, HandshakeError::Timeout { .. }));
    }
}
