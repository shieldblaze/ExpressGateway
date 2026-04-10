//! TLS termination and certificate management for ExpressGateway.
//!
//! This crate provides:
//!
//! - **[`config`]** -- Build `rustls` `ServerConfig` and `ClientConfig` from
//!   ExpressGateway configuration types.
//! - **[`ciphers`]** -- Cipher suite selection based on TLS profile (Modern / Intermediate).
//! - **[`sni`]** -- SNI-based certificate resolution with exact, wildcard, and default fallback.
//! - **[`cert`]** -- PEM certificate and private key loading, X.509 expiry checking.
//! - **[`reload`]** -- Hot-reload support via file watcher with atomic `ServerConfig` swaps.
//! - **[`alpn`]** -- ALPN protocol negotiation (h2 + http/1.1).
//! - **[`mtls`]** -- Mutual TLS client certificate verification.
//! - **[`acceptor`]** -- TLS acceptor wrapper with handshake timeout and metadata extraction.
//! - **[`sniff`]** -- Protocol sniffing: detect TLS ClientHello vs HTTP/1.x vs HTTP/2 preface.
//! - **[`session`]** -- TLS session cache with configurable size and timeout.
//! - **[`error`]** -- Crate-specific error types.

pub mod acceptor;
pub mod alpn;
pub mod cert;
pub mod ciphers;
pub mod config;
pub mod error;
pub mod mtls;
pub mod reload;
pub mod session;
pub mod sniff;
pub mod sni;

// Re-export commonly used types at crate root.
pub use acceptor::{TlsAcceptor, TlsInfo};
pub use cert::CertExpiry;
pub use config::TlsConfigBuilder;
pub use error::TlsError;
pub use reload::TlsReloader;
pub use session::SessionCache;
pub use sniff::{DetectedProtocol, ProtocolSniffer};
pub use sni::SniResolver;
