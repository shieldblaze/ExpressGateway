//! HTTP/2 body size limits: per-stream, per-connection aggregate, and per-stream
//! response body limits.
//!
//! These limits protect against memory exhaustion from large or malicious
//! request/response bodies on HTTP/2 connections.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Default per-connection aggregate body limit (256 MB).
pub const DEFAULT_CONNECTION_BODY_LIMIT: u64 = 256 * 1024 * 1024;

/// Default per-stream body limit (64 MB).
pub const DEFAULT_STREAM_BODY_LIMIT: u64 = 64 * 1024 * 1024;

/// Default per-stream response body limit (128 MB).
pub const DEFAULT_STREAM_RESPONSE_LIMIT: u64 = 128 * 1024 * 1024;

/// Configuration for HTTP/2 body size limits.
#[derive(Debug, Clone)]
pub struct BodyLimitsConfig {
    /// Maximum body size per stream (request or response direction).
    pub per_stream_limit: u64,
    /// Maximum aggregate body size across all streams on a connection.
    pub per_connection_limit: u64,
    /// Maximum response body size per stream.
    pub per_stream_response_limit: u64,
}

impl Default for BodyLimitsConfig {
    fn default() -> Self {
        Self {
            per_stream_limit: DEFAULT_STREAM_BODY_LIMIT,
            per_connection_limit: DEFAULT_CONNECTION_BODY_LIMIT,
            per_stream_response_limit: DEFAULT_STREAM_RESPONSE_LIMIT,
        }
    }
}

/// Error when a body limit is exceeded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BodyLimitError {
    /// Per-stream request body limit exceeded.
    #[error("stream body limit exceeded: {received} > {limit}")]
    StreamLimitExceeded { received: u64, limit: u64 },

    /// Per-connection aggregate body limit exceeded.
    #[error("connection body limit exceeded: {received} > {limit}")]
    ConnectionLimitExceeded { received: u64, limit: u64 },

    /// Per-stream response body limit exceeded.
    #[error("stream response limit exceeded: {received} > {limit}")]
    StreamResponseLimitExceeded { received: u64, limit: u64 },
}

/// Per-connection body tracker shared across all streams on a connection.
///
/// Uses atomic operations for lock-free concurrent access from multiple
/// stream tasks.
pub struct ConnectionBodyTracker {
    /// Total bytes received across all streams.
    total_received: AtomicU64,
    /// Per-connection aggregate limit.
    limit: u64,
}

impl ConnectionBodyTracker {
    /// Create a new connection body tracker with the given aggregate limit.
    pub fn new(limit: u64) -> Self {
        Self {
            total_received: AtomicU64::new(0),
            limit,
        }
    }

    /// Record bytes received and check the aggregate limit.
    ///
    /// Uses a CAS loop to atomically check-then-add, preventing TOCTOU races
    /// where concurrent streams could both pass the limit check before either
    /// observes the other's addition.
    ///
    /// Returns `Ok(new_total)` if the limit is not exceeded, or
    /// `Err(BodyLimitError::ConnectionLimitExceeded)` if it is.
    pub fn record_bytes(&self, bytes: u64) -> Result<u64, BodyLimitError> {
        loop {
            let current = self.total_received.load(Ordering::Acquire);
            let new_total = current + bytes;
            if new_total > self.limit {
                return Err(BodyLimitError::ConnectionLimitExceeded {
                    received: new_total,
                    limit: self.limit,
                });
            }
            if self
                .total_received
                .compare_exchange_weak(current, new_total, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(new_total);
            }
        }
    }

    /// Total bytes received so far.
    pub fn total_received(&self) -> u64 {
        self.total_received.load(Ordering::Acquire)
    }

    /// The configured limit.
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// Per-stream body tracker for request or response direction.
pub struct StreamBodyTracker {
    /// Bytes received on this stream.
    received: u64,
    /// Per-stream limit.
    limit: u64,
    /// Shared connection-level tracker.
    connection_tracker: Arc<ConnectionBodyTracker>,
}

impl StreamBodyTracker {
    /// Create a new stream body tracker.
    pub fn new(limit: u64, connection_tracker: Arc<ConnectionBodyTracker>) -> Self {
        Self {
            received: 0,
            limit,
            connection_tracker,
        }
    }

    /// Record bytes received on this stream and check limits.
    ///
    /// Checks both the per-stream limit and the per-connection aggregate.
    pub fn record_bytes(&mut self, bytes: u64) -> Result<(), BodyLimitError> {
        self.received += bytes;
        if self.received > self.limit {
            return Err(BodyLimitError::StreamLimitExceeded {
                received: self.received,
                limit: self.limit,
            });
        }
        self.connection_tracker.record_bytes(bytes)?;
        Ok(())
    }

    /// Bytes received on this stream so far.
    pub fn received(&self) -> u64 {
        self.received
    }

    /// The configured per-stream limit.
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// Per-stream response body tracker (separate from request tracker).
pub struct StreamResponseTracker {
    /// Bytes sent on this stream response.
    sent: u64,
    /// Per-stream response limit.
    limit: u64,
}

impl StreamResponseTracker {
    /// Create a new response body tracker.
    pub fn new(limit: u64) -> Self {
        Self { sent: 0, limit }
    }

    /// Record bytes sent in the response and check the limit.
    pub fn record_bytes(&mut self, bytes: u64) -> Result<(), BodyLimitError> {
        self.sent += bytes;
        if self.sent > self.limit {
            return Err(BodyLimitError::StreamResponseLimitExceeded {
                received: self.sent,
                limit: self.limit,
            });
        }
        Ok(())
    }

    /// Bytes sent so far.
    pub fn sent(&self) -> u64 {
        self.sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_tracker_allows_within_limit() {
        let tracker = ConnectionBodyTracker::new(1000);
        assert!(tracker.record_bytes(500).is_ok());
        assert!(tracker.record_bytes(499).is_ok());
        assert_eq!(tracker.total_received(), 999);
    }

    #[test]
    fn connection_tracker_rejects_over_limit() {
        let tracker = ConnectionBodyTracker::new(1000);
        assert!(tracker.record_bytes(600).is_ok());
        let err = tracker.record_bytes(600).unwrap_err();
        assert!(matches!(
            err,
            BodyLimitError::ConnectionLimitExceeded { .. }
        ));
    }

    #[test]
    fn stream_tracker_rejects_over_stream_limit() {
        let conn = Arc::new(ConnectionBodyTracker::new(10_000));
        let mut stream = StreamBodyTracker::new(500, conn);
        assert!(stream.record_bytes(300).is_ok());
        let err = stream.record_bytes(300).unwrap_err();
        assert!(matches!(err, BodyLimitError::StreamLimitExceeded { .. }));
    }

    #[test]
    fn stream_tracker_rejects_over_connection_limit() {
        let conn = Arc::new(ConnectionBodyTracker::new(500));
        let mut stream = StreamBodyTracker::new(10_000, conn);
        assert!(stream.record_bytes(300).is_ok());
        let err = stream.record_bytes(300).unwrap_err();
        assert!(matches!(
            err,
            BodyLimitError::ConnectionLimitExceeded { .. }
        ));
    }

    #[test]
    fn response_tracker_allows_within_limit() {
        let mut tracker = StreamResponseTracker::new(1000);
        assert!(tracker.record_bytes(500).is_ok());
        assert!(tracker.record_bytes(499).is_ok());
        assert_eq!(tracker.sent(), 999);
    }

    #[test]
    fn response_tracker_rejects_over_limit() {
        let mut tracker = StreamResponseTracker::new(1000);
        assert!(tracker.record_bytes(800).is_ok());
        let err = tracker.record_bytes(300).unwrap_err();
        assert!(matches!(
            err,
            BodyLimitError::StreamResponseLimitExceeded { .. }
        ));
    }

    #[test]
    fn default_config_values() {
        let config = BodyLimitsConfig::default();
        assert_eq!(config.per_stream_limit, 64 * 1024 * 1024);
        assert_eq!(config.per_connection_limit, 256 * 1024 * 1024);
        assert_eq!(config.per_stream_response_limit, 128 * 1024 * 1024);
    }

    #[test]
    fn multiple_streams_share_connection_budget() {
        let conn = Arc::new(ConnectionBodyTracker::new(1000));
        let mut s1 = StreamBodyTracker::new(800, conn.clone());
        let mut s2 = StreamBodyTracker::new(800, conn.clone());

        assert!(s1.record_bytes(600).is_ok());
        assert!(s2.record_bytes(300).is_ok());
        // Connection total is now 900.

        // s2 tries to add 200 more: 900 + 200 = 1100 > 1000.
        let err = s2.record_bytes(200).unwrap_err();
        assert!(matches!(
            err,
            BodyLimitError::ConnectionLimitExceeded { .. }
        ));
    }
}
