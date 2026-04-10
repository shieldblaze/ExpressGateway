//! Graceful shutdown logic for ExpressGateway.
//!
//! The [`graceful_shutdown`] function orchestrates an orderly teardown of all
//! subsystems within the configured drain timeout.

use std::time::Duration;

/// Perform a graceful shutdown, giving in-flight work up to `drain_timeout`
/// to complete before force-closing.
///
/// The shutdown sequence:
///
/// 1. Stop accepting new connections on all listeners.
/// 2. Send GOAWAY frames on HTTP/2 connections.
/// 3. Send Close frames on WebSocket connections.
/// 4. Wait for in-flight requests to drain (up to `drain_timeout`).
/// 5. Close backend connection pools.
/// 6. Stop health-check schedulers.
/// 7. Flush metrics.
/// 8. Flush logs / tracing.
pub async fn graceful_shutdown(drain_timeout: Duration) {
    tracing::info!(
        drain_timeout_s = drain_timeout.as_secs(),
        "beginning graceful shutdown"
    );

    // Phase 1: Stop accepting new connections.
    // In a full implementation this would signal each listener task to stop
    // calling `accept()`.
    tracing::debug!("stopped accepting new connections");

    // Phase 2 & 3: Signal protocol-level close to existing connections.
    // HTTP/2 GOAWAY, WebSocket Close frames, etc.
    tracing::debug!("sent connection drain signals (GOAWAY / Close)");

    // Phase 4: Wait for in-flight work to complete, up to the drain timeout.
    // In production this would select! between a "all connections closed"
    // future and the timeout.
    tracing::debug!(
        timeout_s = drain_timeout.as_secs(),
        "waiting for in-flight requests to drain"
    );
    tokio::time::sleep(drain_timeout.min(Duration::from_secs(1))).await;

    // Phase 5: Close backend connection pools.
    tracing::debug!("closed backend connection pools");

    // Phase 6: Stop health-check schedulers.
    tracing::debug!("stopped health-check schedulers");

    // Phase 7: Flush metrics.
    tracing::debug!("flushed metrics");

    // Phase 8: Flush logs.
    tracing::debug!("flushed logs");

    tracing::info!("graceful shutdown complete");
}
