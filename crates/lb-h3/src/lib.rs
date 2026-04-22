//! HTTP/3 frame-by-frame codec, QPACK, and QUIC stream handling.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;
mod frame;
mod qpack;
mod security;
mod varint;

pub use error::H3Error;
pub use frame::{DEFAULT_MAX_PAYLOAD_SIZE, H3Frame, decode_frame, encode_frame};
pub use qpack::{QpackDecoder, QpackEncoder};
pub use security::QpackBombDetector;
pub use varint::{MAX_VARINT, decode_varint, encode_varint};
