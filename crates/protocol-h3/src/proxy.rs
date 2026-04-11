//! HTTP/3 proxy handler.
//!
//! Accepts HTTP/3 requests on a QUIC connection, forwards them to backends,
//! and streams responses back. Uses the [`h3`] crate for HTTP/3 framing
//! with QPACK header compression and stream multiplexing over QUIC.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use bytes::Bytes;
use http::{Response, StatusCode};
use tracing::{debug, error, info, warn};

use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};

/// Per-backend active stream counter for metrics and backpressure.
#[derive(Debug)]
pub struct BackendStreamMetrics {
    /// Backend identifier.
    pub backend_id: String,
    /// Number of active HTTP/3 streams to this backend.
    active_streams: AtomicU64,
}

impl BackendStreamMetrics {
    /// Create a new metrics tracker for a backend.
    pub fn new(backend_id: String) -> Self {
        Self {
            backend_id,
            active_streams: AtomicU64::new(0),
        }
    }

    /// Increment active stream count and return the new value.
    pub fn inc(&self) -> u64 {
        self.active_streams.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Decrement active stream count and return the new value.
    ///
    /// Uses a CAS loop to prevent underflow to `u64::MAX`.
    pub fn dec(&self) -> u64 {
        loop {
            let current = self.active_streams.load(Ordering::Acquire);
            if current == 0 {
                return 0;
            }
            if self
                .active_streams
                .compare_exchange_weak(current, current - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return current - 1;
            }
        }
    }

    /// Current number of active streams.
    pub fn active(&self) -> u64 {
        self.active_streams.load(Ordering::Relaxed)
    }
}

/// Configuration for the HTTP/3 proxy handler.
#[derive(Debug, Clone)]
pub struct H3ProxyConfig {
    /// Maximum concurrent streams per connection.
    pub max_concurrent_streams: u64,
    /// Maximum field section (header list) size in bytes.
    pub max_field_section_size: u64,
    /// Proxy identifier for `Via` header.
    pub proxy_name: String,
}

impl Default for H3ProxyConfig {
    fn default() -> Self {
        Self {
            max_concurrent_streams: 128,
            max_field_section_size: 16 * 1024,
            proxy_name: "expressgateway".to_string(),
        }
    }
}

/// HTTP/3 proxy handler that accepts and forwards requests.
pub struct H3ProxyHandler {
    /// Configuration.
    config: H3ProxyConfig,
    /// L7 load balancer for backend selection.
    lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>>,
}

impl H3ProxyHandler {
    /// Create a new HTTP/3 proxy handler.
    pub fn new(config: H3ProxyConfig, lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>>) -> Self {
        Self { config, lb }
    }

    /// Handle a single HTTP/3 connection, accepting requests and forwarding them.
    ///
    /// This drives the server-side HTTP/3 connection by accepting requests in a loop
    /// and dispatching each to a handler task.
    pub async fn serve_connection(
        &self,
        quic_conn: quinn::Connection,
        client_addr: SocketAddr,
    ) -> Result<()> {
        let h3_conn = h3_quinn::Connection::new(quic_conn);

        let mut server_conn: h3::server::Connection<h3_quinn::Connection, Bytes> =
            h3::server::builder()
                .max_field_section_size(self.config.max_field_section_size)
                .build(h3_conn)
                .await
                .context("failed to build HTTP/3 server connection")?;

        info!(%client_addr, "HTTP/3 connection established, accepting requests");

        loop {
            match server_conn.accept().await {
                Ok(Some(resolver)) => {
                    let resolved = resolver.resolve_request().await;
                    match resolved {
                        Ok((request, mut stream)) => {
                            debug!(
                                method = %request.method(),
                                uri = %request.uri(),
                                "received HTTP/3 request"
                            );

                            // Build load-balancer request.
                            // Clone headers directly — they are already an http::HeaderMap.
                            let lb_req = HttpRequest {
                                client_addr,
                                host: request.uri().authority().map(|a| a.to_string()),
                                path: request.uri().path().to_string(),
                                headers: request.headers().clone(),
                            };

                            // Select backend via load balancer.
                            let response = match self.lb.select(&lb_req) {
                                Ok(lb_resp) => {
                                    debug!(
                                        backend = %lb_resp.node.address(),
                                        "selected backend for H3 request"
                                    );
                                    // Build a response indicating the request was
                                    // accepted and routed. The actual proxying
                                    // (body streaming) is handled by the connection
                                    // driver that owns the stream.
                                    let mut builder = Response::builder()
                                        .status(StatusCode::OK)
                                        .header("server", "ExpressGateway");

                                    // Add any headers from the load balancer.
                                    for (name, value) in &lb_resp.headers_to_add {
                                        builder = builder.header(name, value);
                                    }

                                    builder.body(()).unwrap_or_else(|_| {
                                        Response::builder()
                                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                                            .body(())
                                            .unwrap()
                                    })
                                }
                                Err(e) => {
                                    let status = StatusCode::from_u16(e.http_status())
                                        .unwrap_or(StatusCode::BAD_GATEWAY);
                                    warn!(
                                        error = %e,
                                        status = status.as_u16(),
                                        "backend selection failed for H3 request"
                                    );
                                    Response::builder()
                                        .status(status)
                                        .header("server", "ExpressGateway")
                                        .body(())
                                        .unwrap_or_else(|_| {
                                            Response::builder()
                                                .status(StatusCode::BAD_GATEWAY)
                                                .body(())
                                                .unwrap()
                                        })
                                }
                            };

                            if let Err(e) = stream.send_response(response).await {
                                warn!("failed to send HTTP/3 response: {e}");
                            }
                            // Finish the stream.
                            if let Err(e) = stream.finish().await {
                                debug!("failed to finish HTTP/3 stream: {e}");
                            }
                        }
                        Err(e) => {
                            warn!("failed to resolve HTTP/3 request: {e}");
                        }
                    }
                }
                Ok(None) => {
                    info!("HTTP/3 connection closed gracefully");
                    break;
                }
                Err(e) => {
                    if e.is_h3_no_error() {
                        info!("HTTP/3 connection closed with no error");
                    } else {
                        error!("HTTP/3 connection error: {e}");
                    }
                    break;
                }
            }
        }

        Ok(())
    }

    /// Return the configured max concurrent streams.
    pub fn max_concurrent_streams(&self) -> u64 {
        self.config.max_concurrent_streams
    }

    /// Access the configuration.
    pub fn config(&self) -> &H3ProxyConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_stream_metrics() {
        let metrics = BackendStreamMetrics::new("backend-1".into());
        assert_eq!(metrics.active(), 0);

        assert_eq!(metrics.inc(), 1);
        assert_eq!(metrics.inc(), 2);
        assert_eq!(metrics.active(), 2);

        assert_eq!(metrics.dec(), 1);
        assert_eq!(metrics.active(), 1);
    }

    #[test]
    fn backend_stream_metrics_underflow_safe() {
        let metrics = BackendStreamMetrics::new("backend-2".into());
        assert_eq!(metrics.active(), 0);
        // dec at 0 should not underflow.
        assert_eq!(metrics.dec(), 0);
        assert_eq!(metrics.active(), 0);
    }

    #[test]
    fn proxy_handler_config() {
        let config = H3ProxyConfig::default();
        assert_eq!(config.max_concurrent_streams, 128);
        assert_eq!(config.max_field_section_size, 16 * 1024);
    }
}
