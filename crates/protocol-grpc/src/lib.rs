//! gRPC proxy (over HTTP/2 and HTTP/3)
//!
//! This crate provides gRPC-aware proxying: request detection, status mapping,
//! deadline handling, streaming types, trailer forwarding, health checking,
//! compression negotiation, binary framing, and gRPC-Web translation.
//!
//! # Transport combinations supported
//!
//! - gRPC over HTTP/2 <-> HTTP/2
//! - gRPC over HTTP/2 <-> HTTP/3
//! - gRPC over HTTP/3 <-> HTTP/2
//! - gRPC over HTTP/3 <-> HTTP/3
//! - gRPC-Web (HTTP/1.1) <-> native gRPC (HTTP/2 or HTTP/3)

pub mod compression;
pub mod deadline;
pub mod detect;
pub mod error;
pub mod framing;
pub mod grpc_web;
pub mod health;
pub mod status;
pub mod streaming;
pub mod trailers;

pub use compression::{
    EncodingSet, GrpcEncoding, accepted_encodings_from_headers, encoding_from_headers,
    negotiate_encoding, supported_encodings,
};
pub use deadline::{Deadline, DeadlineError, MAX_DEADLINE};
pub use detect::{is_any_grpc, is_grpc, is_grpc_web, is_grpc_web_text};
pub use error::GrpcError;
pub use framing::{
    DEFAULT_MAX_MESSAGE_SIZE, FRAME_HEADER_SIZE, FrameError, FrameHeader, decode_frame,
    encode_frame,
};
pub use grpc_web::{
    GRPC_WEB_CONTENT_TYPE, GRPC_WEB_PROTO_CONTENT_TYPE, GRPC_WEB_TEXT_CONTENT_TYPE,
    decode_trailers_frame, encode_trailers_frame, is_trailer_frame, translate_content_type,
};
pub use health::{HEALTH_CHECK_PATH, HEALTH_WATCH_PATH, health_check_response, is_health_check, is_health_watch};
pub use status::{GrpcStatus, http_to_grpc};
pub use streaming::StreamingType;
pub use trailers::{
    GRPC_TRAILER_KEYS, extract_grpc_trailers, is_grpc_trailers, merge_grpc_trailers,
    parse_grpc_message, parse_grpc_status,
};
