//! HTTP/2 frame-by-frame codec, HPACK, and stream multiplexing.
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
mod hpack;
mod security;

pub use error::H2Error;
pub use frame::{DEFAULT_MAX_FRAME_SIZE, H2Frame, decode_frame, encode_frame};
pub use hpack::{HpackDecoder, HpackEncoder};
pub use security::{ContinuationFloodDetector, HpackBombDetector, RapidResetDetector};
