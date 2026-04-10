//! Crate-specific error types for the TLS module.
//!
//! Uses `thiserror` for structured, non-allocating error enums. Each variant
//! carries enough context to diagnose the failure without dynamic allocation
//! on the error path in most cases.

use std::path::PathBuf;

/// TLS crate error type.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("failed to open file {path}: {source}")]
    FileOpen {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse PEM certificates from {path}")]
    PemCertParse { path: PathBuf },

    #[error("no certificates found in PEM file {path}")]
    NoCertificates { path: PathBuf },

    #[error("failed to parse PEM private key from {path}")]
    PemKeyParse { path: PathBuf },

    #[error("no private key found in PEM file {path}")]
    NoPrivateKey { path: PathBuf },

    #[error("failed to parse X.509 certificate: {reason}")]
    X509Parse { reason: String },

    #[error("invalid timestamp in certificate")]
    InvalidTimestamp,

    #[error("failed to set TLS protocol versions: {reason}")]
    ProtocolVersion { reason: String },

    #[error("failed to set server certificate: {reason}")]
    ServerCert { reason: String },

    #[error("trust CA certificate is required for mTLS {mode} mode")]
    MissingTrustCa { mode: &'static str },

    #[error("failed to add trust CA to root store: {reason}")]
    TrustCaAdd { reason: String },

    #[error("failed to build client certificate verifier: {reason}")]
    ClientVerifier { reason: String },

    #[error("TLS handshake timed out after {timeout_ms}ms")]
    HandshakeTimeout { timeout_ms: u64 },

    #[error("TLS handshake failed: {reason}")]
    HandshakeFailed { reason: String },

    #[error("protocol sniffing failed: insufficient data ({got} bytes, need {need})")]
    SniffInsufficient { got: usize, need: usize },

    #[error("protocol sniffing I/O error: {source}")]
    SniffIo { source: std::io::Error },

    #[error("file watcher error: {reason}")]
    FileWatcher { reason: String },

    #[error("certificate reload failed: {reason}")]
    ReloadFailed { reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Crate-specific Result alias.
pub type Result<T> = std::result::Result<T, TlsError>;
