//! gRPC proxy (over HTTP/2 and HTTP/3)
//!
//! This crate provides gRPC-aware proxying: request detection, status mapping,
//! deadline handling, streaming types, trailer forwarding, health checking,
//! and compression negotiation.

pub mod compression;
pub mod deadline;
pub mod detect;
pub mod health;
pub mod status;
pub mod streaming;
pub mod trailers;

pub use compression::{GrpcEncoding, encoding_from_headers, supported_encodings};
pub use deadline::{Deadline, DeadlineError, MAX_DEADLINE};
pub use detect::is_grpc;
pub use health::{HEALTH_CHECK_PATH, health_check_response, is_health_check};
pub use status::{GrpcStatus, http_to_grpc};
pub use streaming::StreamingType;
pub use trailers::{
    GRPC_TRAILER_KEYS, extract_grpc_trailers, is_grpc_trailers, merge_grpc_trailers,
};
