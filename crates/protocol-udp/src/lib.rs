//! UDP L4 proxy handler for ExpressGateway.
//!
//! This crate implements a UDP reverse proxy that:
//! - Accepts UDP datagrams on a socket
//! - Selects a backend via a pluggable load balancer
//! - Manages sessions (mapping client addresses to backends)
//! - Forwards datagrams bidirectionally via per-session backend sockets
//! - Supports per-source-IP rate limiting
//! - Cleans up expired sessions via a background task
//! - Tracks per-session packet and byte counters

pub mod options;
pub mod proxy;
pub mod session;

pub use options::UdpProxyConfig;
pub use proxy::{UdpProxy, UdpProxyError};
pub use session::{CleanupHandle, SessionManager, UdpSession};
