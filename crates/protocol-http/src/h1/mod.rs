//! HTTP/1.1 reverse proxy implementation.
//!
//! - [`proxy`] -- Main proxy handler with keep-alive and connection pooling.
//! - [`pipelining`] -- HTTP pipelining with serial response ordering.

pub mod pipelining;
pub mod proxy;

pub use pipelining::PipelineQueue;
pub use proxy::{H1ProxyConfig, H1ProxyHandler};
