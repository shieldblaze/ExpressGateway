//! UDP L4 proxy handler.
//!
//! Accepts UDP datagrams on a socket, uses a load balancer for backend
//! selection, manages sessions (mapping client addresses to backends),
//! and forwards datagrams bidirectionally.
//!
//! Each session gets its own dedicated backend socket so that return traffic
//! from the backend can be routed back to the correct client.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use dashmap::DashMap;
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use crate::options::UdpProxyConfig;
use crate::session::SessionManager;

/// Maximum UDP datagram size we will handle (full IPv4 UDP payload).
const MAX_DATAGRAM_SIZE: usize = 65535;

/// Bind address for ephemeral IPv4 sockets.
const EPHEMERAL_V4: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
    0,
);

/// Bind address for ephemeral IPv6 sockets.
const EPHEMERAL_V6: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
    0,
);

/// Errors from the UDP proxy.
#[derive(Debug, thiserror::Error)]
pub enum UdpProxyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("no backend available: {0}")]
    NoBackend(String),
}

/// UDP proxy that forwards datagrams between clients and backends.
///
/// Each client session is assigned a dedicated backend socket bound to an
/// ephemeral port, enabling the proxy to receive return traffic from
/// backends and route it back to the correct client.
pub struct UdpProxy {
    /// Proxy configuration.
    config: UdpProxyConfig,
    /// Load balancer for backend selection.
    lb: Arc<dyn LoadBalancer<L4Request, L4Response>>,
    /// Session manager.
    sessions: Arc<SessionManager>,
    /// Backend sockets: client_addr -> dedicated backend socket.
    /// Each session has its own socket so return traffic is routable.
    backend_sockets: Arc<DashMap<SocketAddr, Arc<UdpSocket>>>,
}

impl UdpProxy {
    /// Create a new UDP proxy with the given config and load balancer.
    pub fn new(config: UdpProxyConfig, lb: Arc<dyn LoadBalancer<L4Request, L4Response>>) -> Self {
        let sessions = Arc::new(SessionManager::new(
            config.session_timeout,
            config.max_sessions,
            config.rate_limit_pps,
        ));
        Self {
            config,
            lb,
            sessions,
            backend_sockets: Arc::new(DashMap::new()),
        }
    }

    /// Get a reference to the session manager.
    #[inline]
    pub fn sessions(&self) -> &Arc<SessionManager> {
        &self.sessions
    }

    /// Get a reference to the config.
    #[inline]
    pub fn config(&self) -> &UdpProxyConfig {
        &self.config
    }

    /// Run the UDP proxy on the given socket.
    ///
    /// This method receives datagrams on `frontend_socket`, selects a backend
    /// (or reuses an existing session), and forwards the datagram.  For each
    /// new session, a dedicated backend socket is created and a background task
    /// is spawned to forward return traffic back to the client.
    pub async fn run(
        &self,
        frontend_socket: Arc<UdpSocket>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), UdpProxyError> {
        let local_addr = frontend_socket.local_addr()?;
        info!(%local_addr, "UDP proxy listening");

        // Start session cleanup background task.
        let cleanup_handle = self.sessions.start_cleanup_task();

        // Use BytesMut for the recv buffer to avoid stack overflow and enable
        // future zero-copy optimizations.
        let mut buf = BytesMut::zeroed(MAX_DATAGRAM_SIZE);
        let mut shutdown = shutdown;

        loop {
            tokio::select! {
                result = frontend_socket.recv_from(&mut buf) => {
                    let (n, client_addr) = match result {
                        Ok(pair) => pair,
                        Err(e) => {
                            error!(error = %e, "Failed to receive UDP datagram");
                            continue;
                        }
                    };

                    self.handle_client_datagram(
                        &frontend_socket,
                        client_addr,
                        &buf[..n],
                    ).await;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("UDP proxy shutting down");
                        break;
                    }
                }
            }
        }

        cleanup_handle.stop();
        info!("UDP proxy shut down");
        Ok(())
    }

    /// Handle a single datagram received from a client.
    async fn handle_client_datagram(
        &self,
        frontend_socket: &UdpSocket,
        client_addr: SocketAddr,
        data: &[u8],
    ) {
        // Rate limit check.
        if !self.sessions.check_rate_limit(&client_addr) {
            debug!(%client_addr, "Rate limited, dropping datagram");
            return;
        }

        // Look up or create a session.
        let backend_addr = if let Some(addr) = self.sessions.get_session(&client_addr) {
            addr
        } else {
            // Select a backend.
            let request = L4Request { client_addr };
            let response = match self.lb.select(&request) {
                Ok(resp) => resp,
                Err(e) => {
                    warn!(error = %e, %client_addr, "No backend available for UDP");
                    return;
                }
            };

            let addr = response.node.address();
            if !self.sessions.create_session(client_addr, addr) {
                warn!(
                    %client_addr,
                    sessions = self.sessions.session_count(),
                    "Session limit reached, dropping datagram"
                );
                return;
            }

            // Create a dedicated backend socket for this session and spawn
            // a return-traffic forwarder.
            if let Err(e) = self
                .setup_backend_socket(frontend_socket, client_addr, addr)
                .await
            {
                warn!(
                    error = %e,
                    %client_addr,
                    %addr,
                    "Failed to create backend socket"
                );
                self.sessions.remove_session(&client_addr);
                return;
            }

            addr
        };

        // Forward to backend via the dedicated backend socket.
        if let Some(socket) = self.backend_sockets.get(&client_addr) {
            match socket.send(data).await {
                Ok(_) => {
                    self.sessions.record_packet(&client_addr, data.len());
                }
                Err(e) => {
                    debug!(
                        error = %e,
                        %client_addr,
                        %backend_addr,
                        "Failed to forward datagram to backend"
                    );
                }
            }
        } else {
            // Backend socket was cleaned up between session lookup and send.
            // This is a transient race; the session will be recreated on the
            // next packet.
            debug!(
                %client_addr,
                %backend_addr,
                "Backend socket missing, session will be recreated"
            );
            self.sessions.remove_session(&client_addr);
        }
    }

    /// Create a dedicated UDP socket connected to the backend and spawn a
    /// task that forwards return traffic back to the client.
    async fn setup_backend_socket(
        &self,
        frontend_socket: &UdpSocket,
        client_addr: SocketAddr,
        backend_addr: SocketAddr,
    ) -> std::io::Result<()> {
        // Bind to ephemeral port, same address family as the backend.
        let bind_addr = if backend_addr.is_ipv4() {
            EPHEMERAL_V4
        } else {
            EPHEMERAL_V6
        };

        let backend_socket = UdpSocket::bind(bind_addr).await?;
        backend_socket.connect(backend_addr).await?;

        let backend_socket = Arc::new(backend_socket);
        self.backend_sockets
            .insert(client_addr, backend_socket.clone());

        // We need the frontend socket's local address to create a new handle
        // for the return-traffic task. Tokio's UdpSocket cannot be cloned, so
        // we re-use the frontend's local address to bind a "send-only" socket
        // with SO_REUSEADDR. This lets the spawned task send datagrams back to
        // the client from the same address the client originally sent to.
        let frontend_local_addr = frontend_socket.local_addr()?;
        let std_socket = socket2::Socket::new(
            if frontend_local_addr.is_ipv4() {
                socket2::Domain::IPV4
            } else {
                socket2::Domain::IPV6
            },
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        std_socket.set_reuse_address(true)?;
        std_socket.set_nonblocking(true)?;
        std_socket.bind(&frontend_local_addr.into())?;
        let reply_socket = Arc::new(UdpSocket::from_std(std_socket.into())?);

        let sessions = self.sessions.clone();
        let backend_sockets = self.backend_sockets.clone();
        let session_timeout = self.config.session_timeout;

        // Spawn a task to forward return traffic from the backend to the client.
        tokio::spawn(async move {
            let mut buf = BytesMut::zeroed(MAX_DATAGRAM_SIZE);
            loop {
                // Use tokio::time::timeout so we stop if the backend goes
                // silent for longer than the session timeout.
                let recv_result = tokio::time::timeout(
                    session_timeout,
                    backend_socket.recv(&mut buf),
                )
                .await;

                match recv_result {
                    Ok(Ok(n)) => {
                        // Check if session is still alive.
                        if sessions.get_session(&client_addr).is_none() {
                            break;
                        }
                        if let Err(e) = reply_socket.send_to(&buf[..n], client_addr).await {
                            debug!(
                                error = %e,
                                %client_addr,
                                "Failed to forward return datagram to client"
                            );
                            break;
                        }
                        sessions.record_packet(&client_addr, n);
                    }
                    Ok(Err(e)) => {
                        debug!(
                            error = %e,
                            %client_addr,
                            "Backend socket recv error"
                        );
                        break;
                    }
                    Err(_elapsed) => {
                        // Session timed out waiting for backend response.
                        debug!(
                            %client_addr,
                            "Return-traffic task timed out, cleaning up"
                        );
                        break;
                    }
                }
            }

            // Cleanup: remove the backend socket entry.
            backend_sockets.remove(&client_addr);
            sessions.remove_session(&client_addr);
            debug!(%client_addr, "Return-traffic forwarder stopped");
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
    use expressgateway_core::node::{Node, NodeImpl};

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
    async fn test_udp_proxy_creation() {
        let node = NodeImpl::new_arc("n1".into(), "127.0.0.1:9999".parse().unwrap(), 1, None);
        let lb: Arc<dyn LoadBalancer<L4Request, L4Response>> = Arc::new(FixedLb { node });
        let config = UdpProxyConfig::default();
        let proxy = UdpProxy::new(config, lb);

        assert_eq!(proxy.sessions().session_count(), 0);
    }

    /// Helper: create a UDP socket with SO_REUSEADDR so that the
    /// return-traffic reply socket can bind to the same address.
    async fn reuseaddr_udp_socket(addr: &str) -> UdpSocket {
        let std_sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        std_sock.set_reuse_address(true).unwrap();
        std_sock.set_nonblocking(true).unwrap();
        let addr: SocketAddr = addr.parse().unwrap();
        std_sock.bind(&addr.into()).unwrap();
        UdpSocket::from_std(std_sock.into()).unwrap()
    }

    #[tokio::test]
    async fn test_handle_client_datagram_creates_session() {
        let backend_addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let node = NodeImpl::new_arc("n1".into(), backend_addr, 1, None);
        let lb: Arc<dyn LoadBalancer<L4Request, L4Response>> = Arc::new(FixedLb { node });
        let config = UdpProxyConfig::default();
        let proxy = UdpProxy::new(config, lb);

        // Bind a UDP socket with SO_REUSEADDR (required for reply socket).
        let socket = reuseaddr_udp_socket("127.0.0.1:0").await;
        let client_addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();

        // Before handling, no session.
        assert!(proxy.sessions().get_session(&client_addr).is_none());

        // handle_client_datagram will create a session.
        proxy
            .handle_client_datagram(&socket, client_addr, b"hello")
            .await;

        // Session should now exist.
        assert_eq!(
            proxy.sessions().get_session(&client_addr),
            Some(backend_addr)
        );
    }

    #[tokio::test]
    async fn test_bidirectional_forwarding() {
        // Set up a real backend that echoes datagrams.
        let backend_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let backend_addr = backend_socket.local_addr().unwrap();

        let node = NodeImpl::new_arc("n1".into(), backend_addr, 1, None);
        let lb: Arc<dyn LoadBalancer<L4Request, L4Response>> = Arc::new(FixedLb { node });
        let config = UdpProxyConfig {
            session_timeout: std::time::Duration::from_secs(5),
            ..UdpProxyConfig::default()
        };
        let proxy = UdpProxy::new(config, lb);

        let frontend_socket = Arc::new(reuseaddr_udp_socket("127.0.0.1:0").await);
        let frontend_addr = frontend_socket.local_addr().unwrap();

        // Client socket to send through the proxy.
        let client_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_addr = client_socket.local_addr().unwrap();

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        // Run the proxy in the background.
        let proxy_handle = {
            let frontend = frontend_socket.clone();
            tokio::spawn(async move { proxy.run(frontend, shutdown_rx).await })
        };

        // Send a datagram from client -> proxy.
        client_socket
            .send_to(b"ping", frontend_addr)
            .await
            .unwrap();

        // The backend should receive the forwarded datagram.
        let mut buf = [0u8; 1024];
        let (n, from_addr) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            backend_socket.recv_from(&mut buf),
        )
        .await
        .expect("backend should receive datagram")
        .unwrap();
        assert_eq!(&buf[..n], b"ping");

        // Backend echoes back.
        backend_socket.send_to(b"pong", from_addr).await.unwrap();

        // Client should receive the return traffic from the proxy's frontend address.
        let (n, reply_addr) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client_socket.recv_from(&mut buf),
        )
        .await
        .expect("client should receive return datagram")
        .unwrap();
        assert_eq!(&buf[..n], b"pong");
        assert_eq!(reply_addr.port(), frontend_addr.port());

        shutdown_tx.send(true).unwrap();
        let _ = proxy_handle.await;
    }

    #[tokio::test]
    async fn test_shutdown() {
        let node = NodeImpl::new_arc("n1".into(), "127.0.0.1:9999".parse().unwrap(), 1, None);
        let lb: Arc<dyn LoadBalancer<L4Request, L4Response>> = Arc::new(FixedLb { node });
        let config = UdpProxyConfig::default();
        let proxy = UdpProxy::new(config, lb);

        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let (tx, rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(async move { proxy.run(socket, rx).await });

        // Signal shutdown.
        tx.send(true).unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}
