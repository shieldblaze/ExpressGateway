//! WebSocket frame handling.
//!
//! Frame types, size limits, and Ping/Pong management per RFC 6455.

use bytes::Bytes;

/// Default maximum frame payload size: 64 KB.
pub const DEFAULT_MAX_FRAME_SIZE: usize = 64 * 1024;

/// WebSocket close code for messages that are too big (RFC 6455 section 7.4.1).
pub const CLOSE_CODE_TOO_BIG: u16 = 1009;

/// WebSocket close code for going away (RFC 6455 section 7.4.1).
pub const CLOSE_CODE_GOING_AWAY: u16 = 1001;

/// WebSocket close code for normal closure (RFC 6455 section 7.4.1).
pub const CLOSE_CODE_NORMAL: u16 = 1000;

/// WebSocket close code for protocol error (RFC 6455 section 7.4.1).
pub const CLOSE_CODE_PROTOCOL_ERROR: u16 = 1002;

/// WebSocket close code for policy violation (RFC 6455 section 7.4.1).
pub const CLOSE_CODE_POLICY_VIOLATION: u16 = 1008;

/// WebSocket frame types per RFC 6455 section 5.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// UTF-8 text data (opcode 0x1).
    Text,
    /// Binary data (opcode 0x2).
    Binary,
    /// Ping control frame (opcode 0x9).
    Ping,
    /// Pong control frame (opcode 0xA).
    Pong,
    /// Close control frame (opcode 0x8).
    Close,
    /// Continuation frame (opcode 0x0).
    Continuation,
}

/// Configuration for frame handling.
#[derive(Debug, Clone)]
pub struct FrameConfig {
    /// Maximum payload size in bytes. Frames exceeding this are rejected
    /// with close code 1009 (Message Too Big).
    pub max_frame_size: usize,

    /// Whether to forward pings to the backend or handle them locally.
    /// When `false` (default), the proxy responds to pings locally with
    /// a pong and does not forward the ping upstream.
    pub forward_pings: bool,
}

impl Default for FrameConfig {
    fn default() -> Self {
        Self {
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            forward_pings: false,
        }
    }
}

impl FrameConfig {
    /// Create a new config with a custom max frame size.
    pub fn with_max_frame_size(max_frame_size: usize) -> Self {
        Self {
            max_frame_size,
            ..Self::default()
        }
    }

    /// Returns `true` if the payload exceeds the configured maximum.
    #[inline]
    pub fn is_oversize(&self, payload_len: usize) -> bool {
        payload_len > self.max_frame_size
    }
}

/// Tracks Ping/Pong state to prevent duplicate Pong responses.
///
/// When the proxy receives a Ping from the client, it responds locally with a
/// Pong and does NOT forward the Ping to the backend. This avoids the backend
/// sending its own Pong that the client doesn't expect.
#[derive(Debug)]
pub struct PingPongTracker {
    /// The last Ping payload we sent a Pong for.
    last_ping_payload: Option<Bytes>,
}

impl PingPongTracker {
    pub fn new() -> Self {
        Self {
            last_ping_payload: None,
        }
    }

    /// Record that we received a Ping with the given payload and should
    /// respond with a Pong locally. Returns the Pong payload (same bytes).
    pub fn handle_ping(&mut self, payload: Bytes) -> Bytes {
        self.last_ping_payload = Some(payload.clone());
        payload
    }

    /// Returns `true` if we should suppress this Pong because we already
    /// sent one for the same payload locally.
    pub fn should_suppress_pong(&self, payload: &[u8]) -> bool {
        self.last_ping_payload
            .as_ref()
            .is_some_and(|p| p.as_ref() == payload)
    }
}

impl Default for PingPongTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_frame_size() {
        let config = FrameConfig::default();
        assert_eq!(config.max_frame_size, 64 * 1024);
        assert!(!config.forward_pings);
    }

    #[test]
    fn custom_max_frame_size() {
        let config = FrameConfig::with_max_frame_size(1024);
        assert_eq!(config.max_frame_size, 1024);
    }

    #[test]
    fn oversize_detection() {
        let config = FrameConfig::with_max_frame_size(100);
        assert!(!config.is_oversize(100));
        assert!(config.is_oversize(101));
        assert!(!config.is_oversize(0));
    }

    #[test]
    fn oversize_at_default_limit() {
        let config = FrameConfig::default();
        assert!(!config.is_oversize(DEFAULT_MAX_FRAME_SIZE));
        assert!(config.is_oversize(DEFAULT_MAX_FRAME_SIZE + 1));
    }

    #[test]
    fn ping_pong_tracker_handles_ping() {
        let mut tracker = PingPongTracker::new();
        let payload = Bytes::from_static(b"ping-data");
        let pong = tracker.handle_ping(payload.clone());
        assert_eq!(pong, payload);
    }

    #[test]
    fn ping_pong_tracker_suppresses_duplicate() {
        let mut tracker = PingPongTracker::new();
        let payload = Bytes::from_static(b"ping-data");
        tracker.handle_ping(payload.clone());
        assert!(tracker.should_suppress_pong(&payload));
    }

    #[test]
    fn ping_pong_tracker_allows_different_pong() {
        let mut tracker = PingPongTracker::new();
        let payload1 = Bytes::from_static(b"ping-1");
        let payload2 = Bytes::from_static(b"ping-2");
        tracker.handle_ping(payload1);
        assert!(!tracker.should_suppress_pong(&payload2));
    }

    #[test]
    fn close_codes() {
        assert_eq!(CLOSE_CODE_TOO_BIG, 1009);
        assert_eq!(CLOSE_CODE_GOING_AWAY, 1001);
        assert_eq!(CLOSE_CODE_NORMAL, 1000);
        assert_eq!(CLOSE_CODE_PROTOCOL_ERROR, 1002);
        assert_eq!(CLOSE_CODE_POLICY_VIOLATION, 1008);
    }
}
