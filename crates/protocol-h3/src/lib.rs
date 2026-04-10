//! HTTP/3 + QUIC L7 proxy for ExpressGateway.
//!
//! This crate provides:
//!
//! - **`quic`** -- QUIC transport layer (RFC 9000) built on [`quinn`].
//! - **`connection`** -- QUIC connection management with migration detection.
//! - **`proxy`** -- HTTP/3 request/response proxying via the [`h3`] crate.
//! - **`retry`** -- Stateless retry token generation and validation.
//! - **`alt_svc`** -- Alt-Svc header injection (RFC 7838) for HTTP/3 advertisement.
//! - **`translation`** -- Protocol translation between H1/H2 and H3.
//! - **`flow_control`** -- QUIC flow control window management.

pub mod alt_svc;
pub mod connection;
pub mod flow_control;
pub mod proxy;
pub mod quic;
pub mod retry;
pub mod translation;

// Re-exports for convenience.
pub use alt_svc::AltSvcHeader;
pub use connection::QuicConnection;
pub use flow_control::FlowController;
pub use proxy::H3ProxyHandler;
pub use quic::{QuicConfig, QuicTransport};
pub use retry::RetryTokenHandler;
pub use translation::{h1_to_h3_request, h3_to_h1_response};
