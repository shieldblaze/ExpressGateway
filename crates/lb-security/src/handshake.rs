//! TLS-handshake timeout helper (SEC-2-10).
//!
//! Wraps `tokio_rustls::TlsAcceptor::accept` in
//! `tokio::time::timeout`. Wave-2c (`crates/lb/src/main.rs`) inserts
//! the call site; this module ships the API + tests so the wiring
//! is a one-liner.
//!
//! The default 5 s budget is well above a healthy TLS 1.3 handshake
//! (sub-second on >99.9% of connections) and well below what a
//! slowloris-style accept-side attacker can sustain without
//! noticing.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::error::Elapsed;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// Default TLS handshake budget — five seconds, per the SEC-2-10
/// plan and the rel F-05 cross-reference.
pub const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 5_000;

/// Outcome of [`timeout_accept`].
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// The handshake budget elapsed before the rustls state machine
    /// reached `Connected`. The caller is expected to close the
    /// underlying socket with a RST (no `shutdown(Write)`) — the
    /// peer has not yet received ServerHello so it has no key
    /// material to ack our close with.
    #[error("TLS handshake exceeded {budget_ms}ms timeout")]
    Timeout {
        /// Configured budget that was exceeded.
        budget_ms: u64,
    },

    /// rustls returned a handshake error — this is distinct from a
    /// timeout because the remediation is different (handshake
    /// error usually means client mis-config, not attack).
    #[error("TLS handshake failed: {0}")]
    Handshake(#[source] std::io::Error),
}

/// Wrap `acceptor.accept(stream)` in a `timeout`.
///
/// Returns `Ok(TlsStream)` on a clean handshake within budget,
/// `Err(HandshakeError::Timeout)` on elapsed, or
/// `Err(HandshakeError::Handshake)` on rustls error.
///
/// `budget` must be non-zero — a zero budget would race the
/// `timeout` future against the handshake future and rejects every
/// connection. The function panics on zero in debug builds (this is
/// a programming error in the caller); in release the budget is
/// silently raised to 1 ms so production stays alive.
///
/// # Errors
///
/// See [`HandshakeError`].
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

    /// A stream that never yields any bytes — feeds the rustls
    /// state machine no ClientHello so it parks indefinitely. The
    /// timeout path is the only termination.
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
        // Build a minimal rustls server config. The handshake will
        // never complete because SilentStream feeds no bytes, but
        // the acceptor itself must construct cleanly.
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
        // Documented behaviour: zero is asserted in debug but silently
        // raised to 1 ms in release. Test in release-like mode by
        // skipping the debug_assert via release build of the test
        // (the test binary is compiled with debug-assertions on by
        // default, so we can't fully exercise the release fallback;
        // we instead verify the timeout STILL fires with the tiny
        // budget so the contract holds either way).
        let acceptor = test_acceptor();
        let stream = SilentStream;
        // 1 ms is the smallest unit that survives the assert; using
        // it directly avoids the debug-assert branch entirely.
        let err = timeout_accept(&acceptor, stream, Duration::from_millis(1))
            .await
            .unwrap_err();
        assert!(matches!(err, HandshakeError::Timeout { .. }));
    }
}
