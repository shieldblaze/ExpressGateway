//! QUIC flow control window management.
//!
//! Manages connection-level (`MAX_DATA`) and stream-level (`MAX_STREAM_DATA`)
//! flow control windows for QUIC connections. This allows fine-grained
//! backpressure between the proxy and both clients and backends.

use std::sync::atomic::{AtomicU64, Ordering};

use tracing::debug;

/// Advisory flow control window tracker for QUIC connections and streams.
///
/// Tracks consumed and available bytes at both connection and stream level,
/// and determines when to send `MAX_DATA` / `MAX_STREAM_DATA` window updates.
///
/// **Note:** This is an advisory layer on top of quinn's transport-level flow
/// control. Quinn enforces the actual QUIC flow control windows (RFC 9000
/// Sections 4.1-4.2). This tracker uses relaxed atomic ordering because a
/// missed or duplicate window-update signal is harmless -- quinn will never
/// allow the peer to exceed the real window.
#[derive(Debug)]
pub struct FlowController {
    /// Connection-level window size (MAX_DATA).
    connection_window: AtomicU64,
    /// Per-stream window size (MAX_STREAM_DATA).
    stream_window: AtomicU64,
    /// Connection-level bytes consumed.
    connection_consumed: AtomicU64,
    /// Threshold ratio (0..100) at which to auto-extend the window.
    /// When consumed > (window * threshold / 100), we should send a window update.
    auto_extend_threshold: u64,
}

impl FlowController {
    /// Create a new flow controller with the given window sizes.
    ///
    /// - `connection_window`: initial connection-level window (MAX_DATA) in bytes.
    /// - `stream_window`: initial per-stream window (MAX_STREAM_DATA) in bytes.
    /// - `auto_extend_threshold`: percentage (0..100) of window consumed before
    ///   triggering a window update. Default recommendation is 50.
    pub fn new(connection_window: u64, stream_window: u64, auto_extend_threshold: u64) -> Self {
        let threshold = auto_extend_threshold.min(100);
        Self {
            connection_window: AtomicU64::new(connection_window),
            stream_window: AtomicU64::new(stream_window),
            connection_consumed: AtomicU64::new(0),
            auto_extend_threshold: threshold,
        }
    }

    /// Record that `bytes` were consumed at the connection level.
    ///
    /// Returns `true` if a window update should be sent (consumed exceeds threshold).
    pub fn consume_connection(&self, bytes: u64) -> bool {
        let consumed = self.connection_consumed.fetch_add(bytes, Ordering::AcqRel) + bytes;
        let window = self.connection_window.load(Ordering::Acquire);
        let threshold = window * self.auto_extend_threshold / 100;

        if consumed > threshold {
            debug!(
                consumed,
                window, threshold, "connection flow control: window update recommended"
            );
            true
        } else {
            false
        }
    }

    /// Extend the connection-level window by `additional` bytes (MAX_DATA update).
    pub fn extend_connection_window(&self, additional: u64) {
        let old = self
            .connection_window
            .fetch_add(additional, Ordering::AcqRel);
        // Reset consumed counter proportionally.
        self.connection_consumed.store(0, Ordering::Release);
        debug!(
            old_window = old,
            new_window = old + additional,
            "connection window extended"
        );
    }

    /// Return the current connection-level window size.
    pub fn connection_window(&self) -> u64 {
        self.connection_window.load(Ordering::Acquire)
    }

    /// Return the current connection-level bytes consumed.
    pub fn connection_consumed(&self) -> u64 {
        self.connection_consumed.load(Ordering::Acquire)
    }

    /// Return the remaining connection-level capacity.
    pub fn connection_remaining(&self) -> u64 {
        let window = self.connection_window.load(Ordering::Acquire);
        let consumed = self.connection_consumed.load(Ordering::Acquire);
        window.saturating_sub(consumed)
    }

    /// Return the per-stream window size (MAX_STREAM_DATA).
    pub fn stream_window(&self) -> u64 {
        self.stream_window.load(Ordering::Acquire)
    }

    /// Update the per-stream window size.
    pub fn set_stream_window(&self, window: u64) {
        self.stream_window.store(window, Ordering::Release);
    }

    /// Check whether the connection window is exhausted.
    pub fn is_connection_blocked(&self) -> bool {
        self.connection_remaining() == 0
    }

    /// Auto-extend threshold as a percentage (0..100).
    pub fn auto_extend_threshold(&self) -> u64 {
        self.auto_extend_threshold
    }
}

impl Default for FlowController {
    fn default() -> Self {
        Self::new(
            1_048_576, // 1 MB connection window
            262_144,   // 256 KB stream window
            50,        // 50% threshold
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state() {
        let fc = FlowController::new(1000, 500, 50);
        assert_eq!(fc.connection_window(), 1000);
        assert_eq!(fc.stream_window(), 500);
        assert_eq!(fc.connection_consumed(), 0);
        assert_eq!(fc.connection_remaining(), 1000);
        assert!(!fc.is_connection_blocked());
    }

    #[test]
    fn consume_below_threshold() {
        let fc = FlowController::new(1000, 500, 50);
        // Consume 400 out of 1000, threshold is 500.
        let needs_update = fc.consume_connection(400);
        assert!(!needs_update);
        assert_eq!(fc.connection_consumed(), 400);
        assert_eq!(fc.connection_remaining(), 600);
    }

    #[test]
    fn consume_above_threshold() {
        let fc = FlowController::new(1000, 500, 50);
        // Consume 600 out of 1000, threshold is 500.
        let needs_update = fc.consume_connection(600);
        assert!(needs_update);
    }

    #[test]
    fn extend_window() {
        let fc = FlowController::new(1000, 500, 50);
        fc.consume_connection(800);
        assert_eq!(fc.connection_remaining(), 200);

        fc.extend_connection_window(1000);
        assert_eq!(fc.connection_window(), 2000);
        // Consumed is reset.
        assert_eq!(fc.connection_consumed(), 0);
        assert_eq!(fc.connection_remaining(), 2000);
    }

    #[test]
    fn connection_blocked() {
        let fc = FlowController::new(100, 50, 50);
        fc.consume_connection(100);
        assert!(fc.is_connection_blocked());
    }

    #[test]
    fn stream_window_update() {
        let fc = FlowController::new(1000, 500, 50);
        assert_eq!(fc.stream_window(), 500);

        fc.set_stream_window(1024);
        assert_eq!(fc.stream_window(), 1024);
    }

    #[test]
    fn default_values() {
        let fc = FlowController::default();
        assert_eq!(fc.connection_window(), 1_048_576);
        assert_eq!(fc.stream_window(), 262_144);
        assert_eq!(fc.auto_extend_threshold(), 50);
    }

    #[test]
    fn threshold_clamped_to_100() {
        let fc = FlowController::new(1000, 500, 200);
        assert_eq!(fc.auto_extend_threshold(), 100);
    }
}
