//! Error types for the HTTP/3 codec.

/// Errors that can occur during HTTP/3 frame parsing, QPACK processing,
/// or security mitigation checks.
#[derive(Debug, thiserror::Error)]
pub enum H3Error {
    /// The input buffer does not contain a complete frame or element.
    #[error("incomplete input, need more data")]
    Incomplete,

    /// A frame type or payload is malformed.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),

    /// A QUIC variable-length integer is malformed.
    #[error("invalid varint encoding")]
    InvalidVarint,

    /// A QPACK encoding or decoding error.
    #[error("QPACK error: {0}")]
    QpackError(String),

    /// A QPACK decompression bomb was detected.
    #[error("QPACK bomb: decoded {decoded} bytes from {encoded} encoded (ratio {ratio})")]
    QpackBomb {
        /// Total decoded header size in bytes.
        decoded: u64,
        /// Total encoded header size in bytes.
        encoded: u64,
        /// Ratio of decoded to encoded.
        ratio: u64,
    },

    /// The frame payload exceeds the configured maximum size.
    #[error("frame too large: {size} exceeds limit {limit}")]
    FrameTooLarge {
        /// Actual frame payload length.
        size: u64,
        /// Configured maximum payload size.
        limit: usize,
    },
}
