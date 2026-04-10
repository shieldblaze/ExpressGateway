//! QUIC transport layer (RFC 9000).
//!
//! Wraps [`quinn::Endpoint`] to provide server and client QUIC transport with
//! configurable stream concurrency, idle timeouts, keep-alive, and 0-RTT.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::rustls;
use tracing::{debug, info};

use crate::connection::QuicConnection;

/// Configuration for QUIC transport parameters.
#[derive(Debug, Clone)]
pub struct QuicConfig {
    /// Maximum number of concurrent bidirectional streams per connection.
    pub max_concurrent_streams: u64,
    /// Maximum idle timeout before the connection is closed.
    pub max_idle_timeout: Duration,
    /// Optional keep-alive interval to prevent NAT rebinding.
    pub keep_alive_interval: Option<Duration>,
    /// Whether to enable 0-RTT data for reduced handshake latency.
    pub enable_0rtt: bool,
}

impl Default for QuicConfig {
    fn default() -> Self {
        Self {
            max_concurrent_streams: 100,
            max_idle_timeout: Duration::from_secs(30),
            keep_alive_interval: Some(Duration::from_secs(15)),
            enable_0rtt: false,
        }
    }
}

/// QUIC transport endpoint supporting both server and client roles.
pub struct QuicTransport {
    endpoint: quinn::Endpoint,
    config: QuicConfig,
}

impl QuicTransport {
    /// Create a server-side QUIC endpoint bound to `bind` with the given TLS configuration.
    pub fn server(bind: SocketAddr, tls_config: rustls::ServerConfig) -> Result<Self> {
        let config = QuicConfig::default();
        Self::server_with_config(bind, tls_config, config)
    }

    /// Create a server-side QUIC endpoint with explicit QUIC config.
    pub fn server_with_config(
        bind: SocketAddr,
        tls_config: rustls::ServerConfig,
        config: QuicConfig,
    ) -> Result<Self> {
        let quic_server_config = QuicServerConfig::try_from(tls_config)
            .map_err(|e| anyhow::anyhow!("failed to create QUIC server crypto config: {e}"))?;

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server_config));
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(
            quinn::VarInt::from_u64(config.max_concurrent_streams)
                .unwrap_or(quinn::VarInt::from_u32(100)),
        );
        transport.max_idle_timeout(Some(
            config
                .max_idle_timeout
                .try_into()
                .context("invalid idle timeout")?,
        ));
        if let Some(interval) = config.keep_alive_interval {
            transport.keep_alive_interval(Some(interval));
        }
        server_config.transport_config(Arc::new(transport));

        let endpoint = quinn::Endpoint::server(server_config, bind)
            .context("failed to create QUIC server endpoint")?;
        info!(%bind, "QUIC server endpoint created");

        Ok(Self { endpoint, config })
    }

    /// Create a client-side QUIC endpoint.
    pub fn client(tls_config: rustls::ClientConfig) -> Result<Self> {
        let config = QuicConfig::default();
        Self::client_with_config(tls_config, config)
    }

    /// Create a client-side QUIC endpoint with explicit QUIC config.
    pub fn client_with_config(
        tls_config: rustls::ClientConfig,
        config: QuicConfig,
    ) -> Result<Self> {
        let quic_client_config = QuicClientConfig::try_from(tls_config)
            .map_err(|e| anyhow::anyhow!("failed to create QUIC client crypto config: {e}"))?;

        let mut client_config = quinn::ClientConfig::new(Arc::new(quic_client_config));
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(
            quinn::VarInt::from_u64(config.max_concurrent_streams)
                .unwrap_or(quinn::VarInt::from_u32(100)),
        );
        transport.max_idle_timeout(Some(
            config
                .max_idle_timeout
                .try_into()
                .context("invalid idle timeout")?,
        ));
        if let Some(interval) = config.keep_alive_interval {
            transport.keep_alive_interval(Some(interval));
        }
        client_config.transport_config(Arc::new(transport));

        let bind_addr: SocketAddr = "[::]:0".parse().unwrap();
        let mut endpoint =
            quinn::Endpoint::client(bind_addr).context("failed to create QUIC client endpoint")?;
        endpoint.set_default_client_config(client_config);
        debug!("QUIC client endpoint created");

        Ok(Self { endpoint, config })
    }

    /// Accept an incoming QUIC connection from a client.
    pub async fn accept(&self) -> Result<QuicConnection> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("endpoint closed"))?;

        let connection = if self.config.enable_0rtt {
            match incoming.accept() {
                Ok(connecting) => match connecting.into_0rtt() {
                    Ok((conn, _zero_rtt_accepted)) => {
                        debug!(
                            remote = %conn.remote_address(),
                            "accepted QUIC connection with 0-RTT"
                        );
                        conn
                    }
                    Err(connecting) => {
                        let conn = connecting.await.context("QUIC handshake failed")?;
                        debug!(
                            remote = %conn.remote_address(),
                            "accepted QUIC connection (0-RTT rejected)"
                        );
                        conn
                    }
                },
                Err(e) => return Err(anyhow::anyhow!("failed to accept incoming: {e}")),
            }
        } else {
            let connecting = incoming
                .accept()
                .map_err(|e| anyhow::anyhow!("failed to accept incoming: {e}"))?;
            let conn = connecting.await.context("QUIC handshake failed")?;
            debug!(remote = %conn.remote_address(), "accepted QUIC connection");
            conn
        };

        Ok(QuicConnection::new(connection))
    }

    /// Connect to a remote QUIC server.
    pub async fn connect(&self, addr: SocketAddr, server_name: &str) -> Result<QuicConnection> {
        let connecting = self
            .endpoint
            .connect(addr, server_name)
            .context("failed to start QUIC connection")?;

        let connection = if self.config.enable_0rtt {
            match connecting.into_0rtt() {
                Ok((conn, _zero_rtt_accepted)) => {
                    debug!(
                        remote = %conn.remote_address(),
                        "connected to QUIC server with 0-RTT"
                    );
                    conn
                }
                Err(connecting) => {
                    let conn = connecting.await.context("QUIC handshake failed")?;
                    debug!(
                        remote = %conn.remote_address(),
                        "connected to QUIC server (0-RTT rejected)"
                    );
                    conn
                }
            }
        } else {
            let conn = connecting.await.context("QUIC handshake failed")?;
            debug!(remote = %conn.remote_address(), "connected to QUIC server");
            conn
        };

        Ok(QuicConnection::new(connection))
    }

    /// Return a reference to the underlying [`quinn::Endpoint`].
    pub fn endpoint(&self) -> &quinn::Endpoint {
        &self.endpoint
    }

    /// Return the QUIC configuration.
    pub fn config(&self) -> &QuicConfig {
        &self.config
    }

    /// Return the local address the endpoint is bound to.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint
            .local_addr()
            .context("failed to get local address")
    }
}
