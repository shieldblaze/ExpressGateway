//! HTTP/1.1 frame-by-frame codec and processing.
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
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

mod chunked;
mod error;
mod parse;

pub use chunked::{ChunkedDecoder, ChunkedEncoder};
pub use error::H1Error;
pub use parse::{parse_headers, parse_request_line, parse_status_line, parse_trailers};
