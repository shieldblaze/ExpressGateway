//! WebSocket proxy
//!
//! This crate provides WebSocket upgrade detection, frame handling,
//! bidirectional proxying, HTTP/2 WebSocket support (RFC 8441),
//! backpressure management, idle timeout handling, origin validation,
//! and subprotocol negotiation forwarding.
//!
//! # Transport combinations supported
//!
//! - HTTP/1.1 Upgrade (RFC 6455) -- standard WebSocket upgrade
//! - HTTP/2 Extended CONNECT (RFC 8441) -- WebSocket over H2
//! - HTTP/2 <-> HTTP/1.1 WebSocket downgrade
//! - HTTP/2 <-> HTTP/2 WebSockets (Extended CONNECT both sides)

pub mod backpressure;
pub mod error;
pub mod frames;
pub mod h2_websocket;
pub mod proxy;
pub mod timeout;
pub mod upgrade;

pub use backpressure::BackpressureState;
pub use error::WsError;
pub use frames::{
    CLOSE_CODE_GOING_AWAY, CLOSE_CODE_NORMAL, CLOSE_CODE_POLICY_VIOLATION,
    CLOSE_CODE_PROTOCOL_ERROR, CLOSE_CODE_TOO_BIG, DEFAULT_MAX_FRAME_SIZE, FrameConfig, FrameType,
    PingPongTracker,
};
pub use h2_websocket::{H2Protocol, H2WebSocketStream, is_h2_websocket_upgrade};
pub use proxy::{ProxyConfig, run_proxy};
pub use timeout::{
    CloseHandshakeTimeout, DEFAULT_CLOSE_HANDSHAKE_TIMEOUT, DEFAULT_HANDSHAKE_TIMEOUT,
    DEFAULT_IDLE_TIMEOUT, HandshakeTimeout, IdleTimeout,
};
pub use upgrade::{
    extract_subprotocols, is_websocket_upgrade, upgrade_response, validate_origin,
};
