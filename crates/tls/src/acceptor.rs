//! TLS acceptor wrapper with handshake timeout and metadata extraction.
//!
//! Wraps `tokio_rustls::TlsAcceptor` with:
//! - Configurable handshake timeout (default 10 seconds) for Slowloris defense
//! - SNI hostname extraction
//! - ALPN negotiated protocol extraction
//! - TLS version and cipher suite reporting

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rustls::ServerConfig;
use tokio::net::TcpStream;
use tracing::debug;

/// Default handshake timeout: 10 seconds (Slowloris defense).
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Information extracted from a completed TLS handshake.
#[derive(Debug, Clone)]
pub struct TlsInfo {
    /// The SNI hostname sent by the client, if any.
    pub sni_hostname: Option<String>,
    /// The ALPN protocol negotiated during the handshake.
    pub alpn_protocol: Option<String>,
    /// The TLS protocol version as a string (e.g., "TLSv1.3").
    pub tls_version: String,
    /// The negotiated cipher suite name.
    pub cipher_suite: String,
    /// The client certificate in DER format, if provided.
    pub client_cert: Option<Vec<u8>>,
}

/// TLS acceptor with handshake timeout and metadata extraction.
pub struct TlsAcceptor {
    inner: tokio_rustls::TlsAcceptor,
    handshake_timeout: Duration,
}

impl TlsAcceptor {
    /// Create a new TLS acceptor with the default 10-second handshake timeout.
    pub fn new(config: Arc<ServerConfig>) -> Self {
        Self {
            inner: tokio_rustls::TlsAcceptor::from(config),
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        }
    }

    /// Create a new TLS acceptor with a custom handshake timeout.
    pub fn with_timeout(config: Arc<ServerConfig>, timeout: Duration) -> Self {
        Self {
            inner: tokio_rustls::TlsAcceptor::from(config),
            handshake_timeout: timeout,
        }
    }

    /// Accept a TLS connection with handshake timeout.
    ///
    /// Returns the TLS stream and extracted handshake metadata.
    /// Fails if the handshake takes longer than the configured timeout
    /// or if the handshake itself fails.
    pub async fn accept(
        &self,
        stream: TcpStream,
    ) -> Result<(tokio_rustls::server::TlsStream<TcpStream>, TlsInfo)> {
        let tls_stream = tokio::time::timeout(self.handshake_timeout, self.inner.accept(stream))
            .await
            .context("TLS handshake timed out")?
            .context("TLS handshake failed")?;

        // Extract metadata from the completed handshake.
        let (_, server_conn) = tls_stream.get_ref();

        let sni_hostname = server_conn.server_name().map(String::from);

        let alpn_protocol = server_conn
            .alpn_protocol()
            .and_then(|p| String::from_utf8(p.to_vec()).ok());

        let tls_version = server_conn
            .protocol_version()
            .map(|v| format!("{v:?}"))
            .unwrap_or_else(|| "unknown".to_string());

        let cipher_suite = server_conn
            .negotiated_cipher_suite()
            .map(|cs| format!("{:?}", cs.suite()))
            .unwrap_or_else(|| "unknown".to_string());

        let client_cert = server_conn
            .peer_certificates()
            .and_then(|certs| certs.first())
            .map(|cert| cert.as_ref().to_vec());

        let info = TlsInfo {
            sni_hostname,
            alpn_protocol,
            tls_version,
            cipher_suite,
            client_cert,
        };

        debug!(
            sni = ?info.sni_hostname,
            alpn = ?info.alpn_protocol,
            version = %info.tls_version,
            cipher = %info.cipher_suite,
            "TLS handshake completed"
        );

        Ok((tls_stream, info))
    }
}
