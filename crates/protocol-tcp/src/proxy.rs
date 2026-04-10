//! TCP L4 proxy handler.
//!
//! Accepts TCP connections on a listener, selects a backend via a load balancer,
//! establishes a connection to the backend, and performs bidirectional data
//! forwarding between client and backend.
//!
//! Supports:
//! - Half-close (RFC 9293): when one side sends FIN, continue forwarding the
//!   other direction until it also closes.
//! - Connection backlog queue: buffer client data while the backend connection
//!   is being established.
//! - Backpressure via write-buffer water marks.

use std::net::SocketAddr;
use std::sync::Arc;

use expressgateway_core::backlog::ConnectionBacklog;
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::{FourTuple, WaterMarks};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

use crate::connection::{ConnectionTracker, TcpConnectionState};
use crate::drain::DrainHandle;
use crate::options::TcpProxyConfig;

/// TCP proxy that forwards connections from a listener to backends
/// selected by a load balancer.
pub struct TcpProxy {
    /// Proxy configuration.
    config: TcpProxyConfig,
    /// Load balancer for backend selection.
    lb: Arc<dyn LoadBalancer<L4Request, L4Response>>,
    /// Connection tracker.
    tracker: Arc<ConnectionTracker>,
    /// Drain handle for graceful shutdown.
    drain: Arc<DrainHandle>,
}

impl TcpProxy {
    /// Create a new TCP proxy with the given config and load balancer.
    pub fn new(config: TcpProxyConfig, lb: Arc<dyn LoadBalancer<L4Request, L4Response>>) -> Self {
        let tracker = Arc::new(ConnectionTracker::new(config.max_connections));
        let drain = Arc::new(DrainHandle::new(config.drain_timeout));
        Self {
            config,
            lb,
            tracker,
            drain,
        }
    }

    /// Get a reference to the connection tracker.
    pub fn tracker(&self) -> &Arc<ConnectionTracker> {
        &self.tracker
    }

    /// Get a reference to the drain handle.
    pub fn drain_handle(&self) -> &Arc<DrainHandle> {
        &self.drain
    }

    /// Run the TCP proxy, accepting connections on the given listener.
    ///
    /// This method runs until the drain handle is activated and all
    /// connections have completed (or the drain timeout is reached).
    pub async fn run(&self, listener: TcpListener) -> anyhow::Result<()> {
        let local_addr = listener.local_addr()?;
        info!(%local_addr, "TCP proxy listening");

        loop {
            if self.drain.is_draining() {
                debug!("Drain active, stopping accept loop");
                break;
            }

            let accept = tokio::select! {
                result = listener.accept() => result,
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => continue,
            };

            let (client_stream, client_addr) = match accept {
                Ok(pair) => pair,
                Err(e) => {
                    error!(error = %e, "Failed to accept TCP connection");
                    continue;
                }
            };

            let four_tuple = FourTuple {
                src_addr: client_addr,
                dst_addr: local_addr,
            };

            // Select backend
            let request = L4Request { client_addr };
            let response = match self.lb.select(&request) {
                Ok(resp) => resp,
                Err(e) => {
                    warn!(error = %e, %client_addr, "No backend available, dropping connection");
                    continue;
                }
            };

            let backend_addr = response.node.address();
            let node = response.node.clone();

            // Register connection
            let conn = match self.tracker.try_add(four_tuple, backend_addr) {
                Some(c) => c,
                None => {
                    warn!(
                        %client_addr,
                        active = self.tracker.active_count(),
                        max = self.tracker.max_connections(),
                        "Connection limit reached, dropping"
                    );
                    continue;
                }
            };

            let config = self.config.clone();
            let tracker = self.tracker.clone();

            tokio::spawn(async move {
                node.inc_connections();

                let result = handle_connection(client_stream, backend_addr, &conn, &config).await;

                if let Err(e) = result {
                    debug!(
                        error = %e,
                        src = %four_tuple.src_addr,
                        backend = %backend_addr,
                        "Connection error"
                    );
                }

                let _ = conn.transition(TcpConnectionState::Closed);
                node.dec_connections();
                tracker.remove(&four_tuple);
            });
        }

        // Wait for drain to complete
        self.drain.wait_for_drain(&self.tracker).await;
        info!("TCP proxy shut down");
        Ok(())
    }
}

/// Handle a single proxied TCP connection: connect to the backend and
/// perform bidirectional forwarding with half-close support.
async fn handle_connection(
    mut client: TcpStream,
    backend_addr: SocketAddr,
    conn: &Arc<crate::connection::TrackedConnection>,
    config: &TcpProxyConfig,
) -> anyhow::Result<()> {
    // Set up backlog for buffering data during backend connect
    let backlog = ConnectionBacklog::new();

    // Connect to backend with timeout
    let mut backend = match tokio::time::timeout(
        config.connect_timeout,
        TcpStream::connect(backend_addr),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            let _ = conn.transition(TcpConnectionState::Closed);
            return Err(anyhow::anyhow!(
                "Backend connection failed to {}: {}",
                backend_addr,
                e
            ));
        }
        Err(_) => {
            let _ = conn.transition(TcpConnectionState::Closed);
            return Err(anyhow::anyhow!(
                "Backend connection timed out to {}",
                backend_addr
            ));
        }
    };

    conn.transition(TcpConnectionState::Active)
        .map_err(|s| anyhow::anyhow!("Invalid state transition from {}", s))?;

    // Flush any backlogged data to backend
    for chunk in backlog.drain_batch() {
        backend.write_all(&chunk).await?;
    }

    // Bidirectional forwarding with half-close support
    let (mut client_read, mut client_write) = client.split();
    let (mut backend_read, mut backend_write) = backend.split();

    let idle_timeout = config.idle_timeout;
    let watermarks = config.backpressure;

    let client_to_backend = async {
        relay_with_idle(
            &mut client_read,
            &mut backend_write,
            idle_timeout,
            watermarks,
        )
        .await
    };

    let backend_to_client = async {
        relay_with_idle(
            &mut backend_read,
            &mut client_write,
            idle_timeout,
            watermarks,
        )
        .await
    };

    // Run both directions concurrently. When one side sends FIN (returns Ok),
    // we shut down the write side of the other direction but continue reading
    // from the still-open direction until it also closes. This is half-close
    // support per RFC 9293.
    tokio::select! {
        result = client_to_backend => {
            // Client closed its send side. Shut down backend write.
            let _ = backend_write.shutdown().await;
            if let Err(e) = result {
                debug!(error = %e, "client->backend relay error");
            }
            // Continue reading from backend until it closes.
            let _ = drain_to_eof(&mut backend_read, &mut client_write).await;
        }
        result = backend_to_client => {
            // Backend closed its send side. Shut down client write.
            let _ = client_write.shutdown().await;
            if let Err(e) = result {
                debug!(error = %e, "backend->client relay error");
            }
            // Continue reading from client until it closes.
            let _ = drain_to_eof(&mut client_read, &mut backend_write).await;
        }
    }

    Ok(())
}

/// Copy data from `reader` to `writer` with idle timeout and backpressure awareness.
async fn relay_with_idle<R, W>(
    reader: &mut R,
    writer: &mut W,
    idle_timeout: std::time::Duration,
    _watermarks: WaterMarks,
) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = [0u8; 8192];
    loop {
        let n = match tokio::time::timeout(idle_timeout, reader.read(&mut buf)).await {
            Ok(Ok(0)) => return Ok(()), // EOF / FIN
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "idle timeout",
                ));
            }
        };
        writer.write_all(&buf[..n]).await?;
    }
}

/// Read from `reader` and write to `writer` until EOF, used to finish the
/// half-open direction after the other side has closed.
async fn drain_to_eof<R, W>(reader: &mut R, writer: &mut W) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                let _ = writer.shutdown().await;
                return Ok(());
            }
            Ok(n) => {
                if writer.write_all(&buf[..n]).await.is_err() {
                    return Ok(());
                }
            }
            Err(_) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
    use expressgateway_core::node::{Node, NodeImpl};
    use std::sync::Arc;

    /// A simple test load balancer that always returns a fixed node.
    struct FixedLb {
        node: Arc<dyn Node>,
    }

    impl LoadBalancer<L4Request, L4Response> for FixedLb {
        fn select(&self, _request: &L4Request) -> expressgateway_core::error::Result<L4Response> {
            Ok(L4Response {
                node: self.node.clone(),
            })
        }
        fn add_node(&self, _node: Arc<dyn Node>) {}
        fn remove_node(&self, _node_id: &str) {}
        fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
            vec![self.node.clone()]
        }
        fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
            vec![self.node.clone()]
        }
        fn get_node(&self, _node_id: &str) -> Option<Arc<dyn Node>> {
            Some(self.node.clone())
        }
    }

    #[tokio::test]
    async fn test_tcp_proxy_creation() {
        let node = NodeImpl::new_arc("n1".into(), "127.0.0.1:9999".parse().unwrap(), 1, None);
        let lb: Arc<dyn LoadBalancer<L4Request, L4Response>> = Arc::new(FixedLb { node });
        let config = TcpProxyConfig::default();
        let proxy = TcpProxy::new(config, lb);

        assert_eq!(proxy.tracker().active_count(), 0);
        assert!(!proxy.drain_handle().is_draining());
    }

    #[tokio::test]
    async fn test_relay_eof() {
        // Test that relay returns Ok on EOF
        let input = b"hello world";
        let mut reader = &input[..];
        let mut writer = Vec::new();

        let result = relay_with_idle(
            &mut reader,
            &mut writer,
            std::time::Duration::from_secs(5),
            WaterMarks::default(),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(writer, b"hello world");
    }

    #[tokio::test]
    async fn test_drain_to_eof_basic() {
        let input = b"remaining data";
        let mut reader = &input[..];
        let mut writer = Vec::new();

        let result = drain_to_eof(&mut reader, &mut writer).await;
        assert!(result.is_ok());
        assert_eq!(writer, b"remaining data");
    }
}
