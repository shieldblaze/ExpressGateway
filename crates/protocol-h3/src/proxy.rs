//! HTTP/3 proxy handler.
//!
//! Accepts HTTP/3 requests on a QUIC connection, forwards them to backends,
//! and streams responses back. Uses the [`h3`] crate for HTTP/3 framing
//! with QPACK header compression and stream multiplexing over QUIC.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use bytes::Bytes;
use http::{Response, StatusCode};
use tracing::{debug, error, info, warn};

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
    pub fn dec(&self) -> u64 {
        self.active_streams.fetch_sub(1, Ordering::Relaxed) - 1
    }

    /// Current number of active streams.
    pub fn active(&self) -> u64 {
        self.active_streams.load(Ordering::Relaxed)
    }
}

/// HTTP/3 proxy handler that accepts and forwards requests.
pub struct H3ProxyHandler {
    /// Maximum concurrent streams per connection.
    max_concurrent_streams: u64,
}

impl H3ProxyHandler {
    /// Create a new HTTP/3 proxy handler.
    pub fn new(max_concurrent_streams: u64) -> Self {
        Self {
            max_concurrent_streams,
        }
    }

    /// Handle a single HTTP/3 connection, accepting requests and forwarding them.
    ///
    /// This drives the server-side HTTP/3 connection by accepting requests in a loop
    /// and dispatching each to a handler task.
    pub async fn serve_connection(&self, quic_conn: quinn::Connection) -> Result<()> {
        let h3_conn = h3_quinn::Connection::new(quic_conn);

        let mut server_conn: h3::server::Connection<h3_quinn::Connection, Bytes> =
            h3::server::builder()
                .max_field_section_size(16 * 1024)
                .build(h3_conn)
                .await
                .context("failed to build HTTP/3 server connection")?;

        info!("HTTP/3 connection established, accepting requests");

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

                            // Build a simple response (proxy logic to be filled in by
                            // integration with the load balancer).
                            let response = Response::builder()
                                .status(StatusCode::BAD_GATEWAY)
                                .header("server", "ExpressGateway")
                                .body(())
                                .unwrap();

                            if let Err(e) = stream.send_response(response).await {
                                warn!("failed to send HTTP/3 response: {e}");
                            }
                            // Finish the stream.
                            let _ = stream.finish().await;
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
        self.max_concurrent_streams
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
    fn proxy_handler_config() {
        let handler = H3ProxyHandler::new(128);
        assert_eq!(handler.max_concurrent_streams(), 128);
    }
}
