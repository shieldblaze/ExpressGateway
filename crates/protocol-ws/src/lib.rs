//! WebSocket proxy
//!
//! This crate provides WebSocket upgrade detection, frame handling,
//! bidirectional proxying, HTTP/2 WebSocket support (RFC 8441),
//! backpressure management, and idle timeout handling.

pub mod backpressure;
pub mod frames;
pub mod h2_websocket;
pub mod proxy;
pub mod timeout;
pub mod upgrade;

pub use backpressure::BackpressureState;
pub use frames::{
    CLOSE_CODE_GOING_AWAY, CLOSE_CODE_NORMAL, CLOSE_CODE_TOO_BIG, DEFAULT_MAX_FRAME_SIZE,
    FrameConfig, FrameType, PingPongTracker,
};
pub use h2_websocket::{H2Protocol, H2WebSocketStream, is_h2_websocket_upgrade};
pub use proxy::{ProxyError, forward_frames, run_proxy};
pub use timeout::{DEFAULT_IDLE_TIMEOUT, IdleTimeout};
pub use upgrade::{is_websocket_upgrade, upgrade_response};
