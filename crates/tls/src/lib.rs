//! TLS termination and certificate management for ExpressGateway.
//!
//! This crate provides:
//!
//! - **[`config`]** -- Build `rustls` `ServerConfig` and `ClientConfig` from
//!   ExpressGateway configuration types.
//! - **[`ciphers`]** -- Cipher suite selection based on TLS profile (Modern / Intermediate).
//! - **[`sni`]** -- SNI-based certificate resolution with exact, wildcard, and default fallback.
//! - **[`cert`]** -- PEM certificate and private key loading, X.509 expiry checking.
//! - **[`reload`]** -- Hot-reload support for atomic `ServerConfig` swaps.
//! - **[`alpn`]** -- ALPN protocol negotiation (h2 + http/1.1).
//! - **[`mtls`]** -- Mutual TLS client certificate verification.
//! - **[`acceptor`]** -- TLS acceptor wrapper with handshake timeout and metadata extraction.

pub mod acceptor;
pub mod alpn;
pub mod cert;
pub mod ciphers;
pub mod config;
pub mod mtls;
pub mod reload;
pub mod sni;

// Re-export commonly used types at crate root.
pub use acceptor::{TlsAcceptor, TlsInfo};
pub use cert::CertExpiry;
pub use config::TlsConfigBuilder;
pub use reload::TlsReloader;
pub use sni::SniResolver;
