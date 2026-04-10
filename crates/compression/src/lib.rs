//! Compression: Brotli, gzip, zstd, deflate.
//!
//! This crate provides compression and decompression for HTTP response bodies,
//! `Accept-Encoding` negotiation, compressible MIME-type detection, and
//! middleware-level decision logic.

pub mod algorithm;
pub mod middleware;
pub mod mime;
pub mod negotiation;

pub use algorithm::{CompressionAlgorithm, CompressionError};
pub use middleware::{CompressionConfig, DEFAULT_MIN_BODY_SIZE, should_compress};
pub use mime::is_compressible;
pub use negotiation::negotiate;
