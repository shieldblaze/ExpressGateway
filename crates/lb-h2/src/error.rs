//! Error types for the HTTP/2 codec.

/// Errors that can occur during HTTP/2 frame parsing, HPACK processing, or
/// security mitigation checks.
#[derive(Debug, thiserror::Error)]
pub enum H2Error {
    /// The input buffer does not contain a complete frame.
    #[error("incomplete input, need more data")]
    Incomplete,

    /// The frame header or payload is malformed.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),

    /// An unknown or unsupported frame type was encountered.
    #[error("unknown frame type: {0}")]
    UnknownFrameType(u8),

    /// An HPACK encoding or decoding error.
    #[error("HPACK error: {0}")]
    HpackError(String),

    /// A rapid-reset attack was detected.
    #[error("rapid reset detected: {count} resets in window")]
    RapidReset {
        /// Number of `RST_STREAM` frames observed in the detection window.
        count: u64,
    },

    /// A CONTINUATION flood was detected.
    #[error("continuation flood: {count} frames without END_HEADERS")]
    ContinuationFlood {
        /// Number of `CONTINUATION` frames without `END_HEADERS`.
        count: u64,
    },

    /// An HPACK bomb was detected (decoded size vastly exceeds encoded size).
    #[error("HPACK bomb: decoded {decoded} bytes from {encoded} encoded (ratio {ratio})")]
    HpackBomb {
        /// Total decoded header size in bytes.
        decoded: u64,
        /// Total encoded header size in bytes.
        encoded: u64,
        /// Ratio of decoded to encoded.
        ratio: u64,
    },

    /// The frame payload exceeds the configured `SETTINGS_MAX_FRAME_SIZE`.
    #[error("frame too large: {size} exceeds limit {limit}")]
    FrameTooLarge {
        /// Actual frame payload length.
        size: usize,
        /// Configured maximum frame size.
        limit: usize,
    },

    /// A `SETTINGS` frame flood was detected.
    #[error("settings flood: {count} SETTINGS frames in window")]
    SettingsFlood {
        /// Number of `SETTINGS` frames observed in the detection window.
        count: u32,
    },

    /// A `PING` frame flood was detected.
    #[error("ping flood: {count} PING frames in window")]
    PingFlood {
        /// Number of `PING` frames observed in the detection window.
        count: u32,
    },

    /// A stream failed to advance under a zero receive-window for longer
    /// than the configured stall timeout.
    #[error("zero-window stall on stream {stream_id}")]
    ZeroWindowStall {
        /// Identifier of the stalled stream.
        stream_id: u32,
    },
}
