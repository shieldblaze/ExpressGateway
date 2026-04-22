//! Error types for HTTP/1.1 codec.

/// Errors that can occur during HTTP/1.1 parsing and encoding.
#[derive(Debug, thiserror::Error)]
pub enum H1Error {
    /// The input buffer does not contain a complete message element.
    #[error("incomplete input, need more data")]
    Incomplete,

    /// The request line is malformed.
    #[error("invalid request line")]
    InvalidRequestLine,

    /// The status line is malformed.
    #[error("invalid status line")]
    InvalidStatusLine,

    /// A header line is malformed.
    #[error("invalid header: {0}")]
    InvalidHeader(String),

    /// The chunk encoding is malformed.
    #[error("invalid chunk encoding")]
    InvalidChunkEncoding,

    /// The body exceeds the configured size limit.
    #[error("body too large: {size} exceeds {limit}")]
    BodyTooLarge {
        /// Actual body size observed.
        size: u64,
        /// Configured maximum.
        limit: u64,
    },
}
