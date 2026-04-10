//! HTTP/2 reverse proxy implementation.
//!
//! - [`proxy`] -- Main proxy handler with stream multiplexing.
//! - [`flow_control`] -- Connection-level and stream-level flow control.
//! - [`h2c`] -- HTTP/2 cleartext (h2c) detection and upgrade.
//! - [`connect`] -- CONNECT tunneling per RFC 9113 Section 8.5.

pub mod connect;
pub mod flow_control;
pub mod h2c;
pub mod proxy;

pub use connect::ConnectRequest;
pub use flow_control::{FlowController, FlowWindow};
pub use h2c::{H2C_PREFACE, detect_h2c_preface};
pub use proxy::{H2ProxyConfig, H2ProxyHandler};
