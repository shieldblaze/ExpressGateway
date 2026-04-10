//! TCP L4 proxy handler for ExpressGateway.
//!
//! This crate implements a TCP reverse proxy that:
//! - Accepts TCP connections on a listener
//! - Selects a backend via a pluggable load balancer
//! - Performs bidirectional data forwarding (client <-> backend)
//! - Supports half-close (RFC 9293)
//! - Tracks connections through a state machine
//! - Provides graceful drain with configurable timeout
//! - Applies configurable TCP socket options (nodelay, keepalive, quickack, fastopen)
//! - Enforces backpressure via write-buffer water marks
//! - Tracks per-connection byte counters for observability

pub mod connection;
pub mod drain;
pub mod options;
pub mod proxy;

pub use connection::{ConnectionTracker, TcpConnectionState, TrackedConnection};
pub use drain::DrainHandle;
pub use options::{TcpKeepalive, TcpProxyConfig, TcpSocketOptions};
pub use proxy::{TcpProxy, TcpProxyError};
