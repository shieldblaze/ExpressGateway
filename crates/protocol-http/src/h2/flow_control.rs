//! HTTP/2 flow control management.
//!
//! Tracks connection-level and stream-level flow control windows, and
//! propagates backpressure when windows are exhausted.

use std::sync::atomic::{AtomicI64, Ordering};

/// Default initial window size (65,535 bytes per RFC 9113).
pub const DEFAULT_INITIAL_WINDOW_SIZE: i64 = 65_535;

/// Default connection-level window size (1 MB).
pub const DEFAULT_CONNECTION_WINDOW_SIZE: i64 = 1_048_576;

/// Maximum flow control window size per RFC 9113 §6.9.1 (2^31 - 1).
pub const MAX_FLOW_CONTROL_WINDOW: i64 = (1 << 31) - 1;

/// A flow control window tracking available send/receive capacity.
///
/// Uses atomic operations for lock-free concurrent access.
#[derive(Debug)]
pub struct FlowWindow {
    /// Available window size in bytes (can go negative if overcommitted).
    available: AtomicI64,
    /// Initial window size for resets.
    initial_size: i64,
}

impl FlowWindow {
    /// Create a new flow window with the given initial size.
    pub fn new(initial_size: i64) -> Self {
        Self {
            available: AtomicI64::new(initial_size),
            initial_size,
        }
    }

    /// Try to consume `bytes` from the window.
    ///
    /// Returns `true` if the bytes were consumed, `false` if insufficient
    /// window capacity (backpressure signal).
    pub fn try_consume(&self, bytes: i64) -> bool {
        loop {
            let current = self.available.load(Ordering::Acquire);
            if current < bytes {
                return false;
            }
            if self
                .available
                .compare_exchange_weak(
                    current,
                    current - bytes,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Add capacity back to the window (e.g., after receiving WINDOW_UPDATE).
    ///
    /// Per RFC 9113 §6.9.1, the flow control window MUST NOT exceed 2^31 - 1.
    /// Returns `true` if the replenishment was applied, `false` if it would
    /// overflow the maximum window size (a FLOW_CONTROL_ERROR).
    pub fn replenish(&self, bytes: i64) -> bool {
        loop {
            let current = self.available.load(Ordering::Acquire);
            let new_val = current.saturating_add(bytes);
            if new_val > MAX_FLOW_CONTROL_WINDOW {
                return false;
            }
            if self
                .available
                .compare_exchange_weak(current, new_val, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Current available window size.
    pub fn available(&self) -> i64 {
        self.available.load(Ordering::Acquire)
    }

    /// Reset the window to its initial size.
    pub fn reset(&self) {
        self.available.store(self.initial_size, Ordering::Release);
    }

    /// Whether the window is exhausted (zero or negative).
    pub fn is_exhausted(&self) -> bool {
        self.available.load(Ordering::Acquire) <= 0
    }
}

/// HTTP/2 flow controller managing connection-level and stream-level windows.
///
/// Enforces backpressure: if either the connection window or the stream window
/// is exhausted, data transmission must pause until a WINDOW_UPDATE is received.
pub struct FlowController {
    /// Connection-level flow control window (shared by all streams).
    connection_window: FlowWindow,
    /// Default initial window size for new streams.
    default_stream_window_size: i64,
}

impl FlowController {
    /// Create a new flow controller with the given connection window size and
    /// default per-stream window size.
    pub fn new(connection_window_size: i64, stream_window_size: i64) -> Self {
        Self {
            connection_window: FlowWindow::new(connection_window_size),
            default_stream_window_size: stream_window_size,
        }
    }

    /// Create a flow controller with default RFC 9113 settings.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_CONNECTION_WINDOW_SIZE, DEFAULT_INITIAL_WINDOW_SIZE)
    }

    /// Try to send `bytes` on a given stream.
    ///
    /// Checks both the connection-level and stream-level windows. Returns `true`
    /// if the send is allowed, `false` if backpressure should be applied.
    pub fn try_send(&self, stream_window: &FlowWindow, bytes: i64) -> bool {
        // Check connection-level window first.
        if !self.connection_window.try_consume(bytes) {
            return false;
        }
        // Check stream-level window.
        if !stream_window.try_consume(bytes) {
            // Rollback connection-level consumption.
            // This replenish cannot overflow because we just consumed these bytes.
            let _ = self.connection_window.replenish(bytes);
            return false;
        }
        true
    }

    /// Handle a connection-level WINDOW_UPDATE.
    ///
    /// Returns `true` if the update was applied, `false` if it would exceed
    /// the maximum flow control window (RFC 9113 §6.9.1 FLOW_CONTROL_ERROR).
    pub fn connection_window_update(&self, increment: i64) -> bool {
        self.connection_window.replenish(increment)
    }

    /// Create a new stream-level flow window with the default initial size.
    pub fn new_stream_window(&self) -> FlowWindow {
        FlowWindow::new(self.default_stream_window_size)
    }

    /// Current available connection-level window.
    pub fn connection_window_available(&self) -> i64 {
        self.connection_window.available()
    }

    /// Whether the connection-level window is exhausted.
    pub fn is_connection_blocked(&self) -> bool {
        self.connection_window.is_exhausted()
    }
}

impl Default for FlowController {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_window_consume_and_replenish() {
        let w = FlowWindow::new(100);
        assert_eq!(w.available(), 100);

        assert!(w.try_consume(60));
        assert_eq!(w.available(), 40);

        assert!(!w.try_consume(50)); // Not enough.
        assert_eq!(w.available(), 40);

        assert!(w.replenish(30));
        assert_eq!(w.available(), 70);
    }

    #[test]
    fn flow_window_exhaustion() {
        let w = FlowWindow::new(10);
        assert!(!w.is_exhausted());
        assert!(w.try_consume(10));
        assert!(w.is_exhausted());
    }

    #[test]
    fn flow_window_reset() {
        let w = FlowWindow::new(100);
        w.try_consume(80);
        assert_eq!(w.available(), 20);
        w.reset();
        assert_eq!(w.available(), 100);
    }

    #[test]
    fn flow_controller_dual_window_check() {
        let fc = FlowController::new(1000, 100);
        let sw = fc.new_stream_window();

        // Both windows have capacity.
        assert!(fc.try_send(&sw, 50));
        assert_eq!(sw.available(), 50);
        assert_eq!(fc.connection_window_available(), 950);

        // Stream window blocks.
        assert!(!fc.try_send(&sw, 60));
        // Connection window should be rolled back.
        assert_eq!(fc.connection_window_available(), 950);
    }

    #[test]
    fn flow_controller_connection_window_blocks() {
        let fc = FlowController::new(50, 1000);
        let sw = fc.new_stream_window();

        assert!(fc.try_send(&sw, 50));
        assert!(!fc.try_send(&sw, 1)); // Connection window exhausted.
        assert!(fc.is_connection_blocked());

        // Replenish connection window.
        assert!(fc.connection_window_update(100));
        assert!(!fc.is_connection_blocked());
        assert!(fc.try_send(&sw, 50));
    }

    #[test]
    fn flow_window_replenish_overflow_rejected() {
        // RFC 9113 §6.9.1: window MUST NOT exceed 2^31 - 1.
        let w = FlowWindow::new(MAX_FLOW_CONTROL_WINDOW - 10);
        assert!(w.replenish(10)); // Exactly at max.
        assert!(!w.replenish(1)); // Would exceed max.
    }

    #[test]
    fn connection_window_update_overflow_rejected() {
        let fc = FlowController::new(MAX_FLOW_CONTROL_WINDOW - 5, 65_535);
        assert!(fc.connection_window_update(5));
        assert!(!fc.connection_window_update(1));
    }

    #[test]
    fn flow_controller_defaults() {
        let fc = FlowController::with_defaults();
        assert_eq!(
            fc.connection_window_available(),
            DEFAULT_CONNECTION_WINDOW_SIZE
        );
        let sw = fc.new_stream_window();
        assert_eq!(sw.available(), DEFAULT_INITIAL_WINDOW_SIZE);
    }
}
