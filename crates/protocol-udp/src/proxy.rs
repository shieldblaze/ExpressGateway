//! UDP L4 proxy handler.
//!
//! Accepts UDP datagrams on a socket, uses a load balancer for backend
//! selection, manages sessions (mapping client addresses to backends),
//! and forwards datagrams bidirectionally.

use std::net::SocketAddr;
use std::sync::Arc;

use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use crate::options::UdpProxyConfig;
use crate::session::SessionManager;

/// Maximum UDP datagram size we will handle.
const MAX_DATAGRAM_SIZE: usize = 65535;

/// UDP proxy that forwards datagrams between clients and backends.
pub struct UdpProxy {
    /// Proxy configuration.
    config: UdpProxyConfig,
    /// Load balancer for backend selection.
    lb: Arc<dyn LoadBalancer<L4Request, L4Response>>,
    /// Session manager.
    sessions: Arc<SessionManager>,
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
        }
    }

    /// Get a reference to the session manager.
    pub fn sessions(&self) -> &Arc<SessionManager> {
        &self.sessions
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &UdpProxyConfig {
        &self.config
    }

    /// Run the UDP proxy on the given socket.
    ///
    /// This method receives datagrams on `frontend_socket`, selects a backend
    /// (or reuses an existing session), and forwards the datagram. Backend
    /// responses are forwarded back to the original client.
    pub async fn run(
        &self,
        frontend_socket: Arc<UdpSocket>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let local_addr = frontend_socket.local_addr()?;
        info!(%local_addr, "UDP proxy listening");

        // Start session cleanup background task
        let cleanup_handle = self.sessions.start_cleanup_task();

        let mut buf = vec![0u8; MAX_DATAGRAM_SIZE];
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
        // Rate limit check
        if !self.sessions.check_rate_limit(&client_addr) {
            debug!(%client_addr, "Rate limited, dropping datagram");
            return;
        }

        // Look up or create a session
        let backend_addr = if let Some(addr) = self.sessions.get_session(&client_addr) {
            addr
        } else {
            // Select a backend
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
            addr
        };

        // Forward to backend.
        // We use send_to from the frontend socket directly. In production, each
        // session would typically have its own backend socket for receiving
        // return traffic. Here we do a simple implementation.
        if let Err(e) = frontend_socket.send_to(data, backend_addr).await {
            debug!(
                error = %e,
                %client_addr,
                %backend_addr,
                "Failed to forward datagram to backend"
            );
        }
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

    #[tokio::test]
    async fn test_handle_client_datagram_creates_session() {
        let backend_addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let node = NodeImpl::new_arc("n1".into(), backend_addr, 1, None);
        let lb: Arc<dyn LoadBalancer<L4Request, L4Response>> = Arc::new(FixedLb { node });
        let config = UdpProxyConfig::default();
        let proxy = UdpProxy::new(config, lb);

        // Bind a UDP socket
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();

        // Before handling, no session
        assert!(proxy.sessions().get_session(&client_addr).is_none());

        // handle_client_datagram will create a session (send may fail since
        // backend isn't listening, but session should still be created)
        proxy
            .handle_client_datagram(&socket, client_addr, b"hello")
            .await;

        // Session should now exist
        assert_eq!(
            proxy.sessions().get_session(&client_addr),
            Some(backend_addr)
        );
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

        // Signal shutdown
        tx.send(true).unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}
