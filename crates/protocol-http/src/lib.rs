//! HTTP/1.1 + HTTP/2 reverse proxy for ExpressGateway.
//!
//! This crate provides a comprehensive L7 HTTP reverse proxy implementation
//! supporting both HTTP/1.1 and HTTP/2, with:
//!
//! - **HTTP/1.1 proxy** ([`h1`]): keep-alive connection pooling, chunked transfer
//!   encoding, request body streaming, and HTTP pipelining.
//! - **HTTP/2 proxy** ([`h2`]): stream multiplexing, per-stream load balancing,
//!   flow control, h2c cleartext detection, and CONNECT tunneling.
//! - **Header manipulation** ([`headers`]): hop-by-hop stripping (RFC 9110/9113),
//!   proxy header injection (Via, X-Request-ID, X-Forwarded-For/Proto).
//! - **URI normalization** ([`uri`]): dot-segment removal (RFC 3986), path
//!   traversal detection, null byte and double-encoded dot rejection.
//! - **Body handling** ([`body`]): per-request size limits, Slowloris defense
//!   header timeout, slow-POST body timeout.
//! - **Body limits** ([`body_limits`]): per-stream and per-connection aggregate
//!   body size limits for HTTP/2.
//! - **Error responses** ([`errors`]): structured JSON error responses for
//!   400, 413, 414, 431, 502, 503, 504.
//! - **Health endpoints** ([`health`]): `/health` and `/ready` handlers.
//! - **Protocol translation** ([`translation`]): H1<->H2 header and body conversion.
//! - **Flood protection** ([`flood_protection`]): HTTP/2 SETTINGS/PING rate limiting.

pub mod body;
pub mod body_limits;
pub mod errors;
pub mod flood_protection;
pub mod h1;
pub mod h2;
pub mod headers;
pub mod health;
pub mod translation;
pub mod uri;

// Re-export primary types for convenience.
pub use body::{BodyConfig, LimitedBody};
pub use body_limits::{BodyLimitsConfig, ConnectionBodyTracker, StreamBodyTracker};
pub use errors::HttpError;
pub use flood_protection::{ControlFrameRateLimiter, ControlFrameType, ENHANCE_YOUR_CALM};
pub use h1::{H1ProxyConfig, H1ProxyHandler, PipelineQueue};
pub use h2::{ConnectRequest, FlowController, FlowWindow, H2ProxyConfig, H2ProxyHandler};
pub use headers::{inject_proxy_headers, strip_hop_by_hop};
pub use health::{HealthState, handle_health, handle_ready};
pub use translation::{H1BodyEncoding, H2PseudoHeaders, h1_to_h2_headers, h2_to_h1_request};
pub use uri::{NormalizedUri, UriError, normalize};
