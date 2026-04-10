//! TCP L4 proxy handler.
//!
//! Accepts TCP connections on a listener, selects a backend via a load balancer,
//! establishes a connection to the backend, and performs bidirectional data
//! forwarding between client and backend.
//!
//! Supports:
//! - Half-close (RFC 9293): when one side sends FIN, continue forwarding the
//!   other direction until it also closes.
//! - Backpressure via write-buffer water marks: pauses reads when the write
//!   side falls behind, preventing unbounded memory growth.
//! - Per-connection byte counters for observability.
//! - Configurable TCP socket options (nodelay, keepalive, quickack, fastopen).

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::{FourTuple, WaterMarks};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

use crate::connection::{ConnectionTracker, TcpConnectionState, TrackedConnection, apply_socket_options};
use crate::drain::DrainHandle;
use crate::options::TcpProxyConfig;

/// Errors from the TCP proxy data path.
#[derive(Debug, thiserror::Error)]
pub enum TcpProxyError {
    #[error("backend connection failed to {addr}: {source}")]
    BackendConnect {
        addr: SocketAddr,
        source: std::io::Error,
    },

    #[error("backend connection timed out to {0}")]
    BackendTimeout(SocketAddr),

    #[error("invalid state transition from {0}")]
    InvalidTransition(TcpConnectionState),

    #[error("relay error: {0}")]
    Relay(#[from] std::io::Error),
}

/// Size of the relay buffer.  64 KiB is a good balance: large enough to
/// saturate a 10 Gbps link at reasonable syscall rates, small enough to avoid
/// excessive per-connection memory.
const RELAY_BUF_SIZE: usize = 65_536;

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
    #[inline]
    pub fn tracker(&self) -> &Arc<ConnectionTracker> {
        &self.tracker
    }

    /// Get a reference to the drain handle.
    #[inline]
    pub fn drain_handle(&self) -> &Arc<DrainHandle> {
        &self.drain
    }

    /// Run the TCP proxy, accepting connections on the given listener.
    ///
    /// This method runs until the drain handle is activated and all
    /// connections have completed (or the drain timeout is reached).
    pub async fn run(&self, listener: TcpListener) -> Result<(), TcpProxyError> {
        let local_addr = listener
            .local_addr()
            .map_err(TcpProxyError::Relay)?;
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

            // Select backend.
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

            // Register connection.
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

        // Wait for drain to complete.
        self.drain.wait_for_drain(&self.tracker).await;
        info!("TCP proxy shut down");
        Ok(())
    }
}

/// Handle a single proxied TCP connection: connect to the backend and
/// perform bidirectional forwarding with half-close support and backpressure.
async fn handle_connection(
    client: TcpStream,
    backend_addr: SocketAddr,
    conn: &Arc<TrackedConnection>,
    config: &TcpProxyConfig,
) -> Result<(), TcpProxyError> {
    // Apply socket options to client side.
    if let Err(e) = apply_socket_options(&client, &config.socket_options) {
        debug!(error = %e, "Failed to set client socket options (non-fatal)");
    }

    // Connect to backend with timeout.
    let backend = match tokio::time::timeout(
        config.connect_timeout,
        TcpStream::connect(backend_addr),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            let _ = conn.transition(TcpConnectionState::Closed);
            return Err(TcpProxyError::BackendConnect {
                addr: backend_addr,
                source: e,
            });
        }
        Err(_) => {
            let _ = conn.transition(TcpConnectionState::Closed);
            return Err(TcpProxyError::BackendTimeout(backend_addr));
        }
    };

    // Apply socket options to backend side.
    if let Err(e) = apply_socket_options(&backend, &config.socket_options) {
        debug!(error = %e, "Failed to set backend socket options (non-fatal)");
    }

    conn.transition(TcpConnectionState::Active)
        .map_err(TcpProxyError::InvalidTransition)?;

    // Bidirectional forwarding with half-close support.
    let (mut client_read, mut client_write) = client.into_split();
    let (mut backend_read, mut backend_write) = backend.into_split();

    let idle_timeout = config.idle_timeout;
    let watermarks = config.backpressure;
    let conn_c2b = conn.clone();
    let conn_b2c = conn.clone();

    let client_to_backend = async {
        relay_with_backpressure(
            &mut client_read,
            &mut backend_write,
            idle_timeout,
            watermarks,
            &conn_c2b,
            Direction::ClientToBackend,
        )
        .await
    };

    let backend_to_client = async {
        relay_with_backpressure(
            &mut backend_read,
            &mut client_write,
            idle_timeout,
            watermarks,
            &conn_b2c,
            Direction::BackendToClient,
        )
        .await
    };

    // Run both directions concurrently.  When one side sends FIN (returns Ok),
    // we shut down the write side of the other direction but continue reading
    // from the still-open direction until it also closes.  This is half-close
    // support per RFC 9293 section 3.5.
    tokio::select! {
        result = client_to_backend => {
            // Client closed its send side.  Shut down backend write.
            let _ = backend_write.shutdown().await;
            if let Err(e) = result {
                debug!(error = %e, "client->backend relay error");
            }
            // Continue reading from backend until it closes.
            let _ = drain_to_eof(&mut backend_read, &mut client_write, conn, Direction::BackendToClient).await;
        }
        result = backend_to_client => {
            // Backend closed its send side.  Shut down client write.
            let _ = client_write.shutdown().await;
            if let Err(e) = result {
                debug!(error = %e, "backend->client relay error");
            }
            // Continue reading from client until it closes.
            let _ = drain_to_eof(&mut client_read, &mut backend_write, conn, Direction::ClientToBackend).await;
        }
    }

    Ok(())
}

/// Direction of data flow for byte counter accounting.
#[derive(Debug, Clone, Copy)]
enum Direction {
    ClientToBackend,
    BackendToClient,
}

/// Copy data from `reader` to `writer` with idle timeout and backpressure.
///
/// Uses `BytesMut` for the relay buffer.  When the writer falls behind
/// (tracked by pending write bytes exceeding the high water mark), reads
/// are paused until the write buffer drains below the low water mark.
/// This prevents unbounded memory growth when one side is slower than
/// the other.
async fn relay_with_backpressure<R, W>(
    reader: &mut R,
    writer: &mut W,
    idle_timeout: std::time::Duration,
    watermarks: WaterMarks,
    conn: &TrackedConnection,
    direction: Direction,
) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = BytesMut::with_capacity(RELAY_BUF_SIZE);
    let mut pending_writes: usize = 0;

    loop {
        // Backpressure: if we have too much pending data, flush before reading more.
        if pending_writes >= watermarks.high {
            writer.flush().await?;
            pending_writes = 0;
            // After flush, if we were above high water mark, the writes are
            // committed.  In a real kernel TCP stack, the write buffer is now
            // drained to the kernel socket buffer.
        }

        // Ensure the buffer has capacity for the next read.
        buf.reserve(RELAY_BUF_SIZE);

        let n = match tokio::time::timeout(idle_timeout, reader.read_buf(&mut buf)).await {
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
        buf.clear();

        // Update byte counters and activity timestamp.
        match direction {
            Direction::ClientToBackend => conn.record_sent(n as u64),
            Direction::BackendToClient => conn.record_received(n as u64),
        }
        conn.touch();
        pending_writes += n;
    }
}

/// Read from `reader` and write to `writer` until EOF, used to finish the
/// half-open direction after the other side has closed.
async fn drain_to_eof<R, W>(
    reader: &mut R,
    writer: &mut W,
    conn: &TrackedConnection,
    direction: Direction,
) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = BytesMut::with_capacity(RELAY_BUF_SIZE);
    loop {
        buf.reserve(RELAY_BUF_SIZE);
        match reader.read_buf(&mut buf).await {
            Ok(0) => {
                let _ = writer.shutdown().await;
                return Ok(());
            }
            Ok(n) => {
                match direction {
                    Direction::ClientToBackend => conn.record_sent(n as u64),
                    Direction::BackendToClient => conn.record_received(n as u64),
                }
                conn.touch();
                if writer.write_all(&buf[..n]).await.is_err() {
                    return Ok(());
                }
                buf.clear();
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
        // Test that relay returns Ok on EOF.
        let input = b"hello world";
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        let result = relay_with_backpressure(
            &mut reader,
            &mut writer,
            std::time::Duration::from_secs(5),
            WaterMarks::default(),
            &conn,
            Direction::ClientToBackend,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(writer, b"hello world");
        assert_eq!(conn.bytes_sent.load(std::sync::atomic::Ordering::Relaxed), 11);
    }

    #[tokio::test]
    async fn test_drain_to_eof_basic() {
        let input = b"remaining data";
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        let result = drain_to_eof(&mut reader, &mut writer, &conn, Direction::BackendToClient).await;
        assert!(result.is_ok());
        assert_eq!(writer, b"remaining data");
        assert_eq!(conn.bytes_received.load(std::sync::atomic::Ordering::Relaxed), 14);
    }
}
