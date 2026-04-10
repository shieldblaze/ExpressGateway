//! Crate-specific error types for the XDP module.

/// XDP crate error type.
#[derive(Debug, thiserror::Error)]
pub enum XdpError {
    #[error("XDP is not available on this platform")]
    PlatformUnavailable,

    #[error("XDP program is already attached to {interface}")]
    AlreadyAttached { interface: String },

    #[error("XDP program is not attached to {interface}")]
    NotAttached { interface: String },

    #[error("failed to load BPF program: {reason}")]
    LoadFailed { reason: String },

    #[error("failed to attach XDP program to {interface}: {reason}")]
    AttachFailed { interface: String, reason: String },

    #[error("failed to detach XDP program from {interface}: {reason}")]
    DetachFailed { interface: String, reason: String },

    #[error("BPF map operation failed on {map_name}: {reason}")]
    MapError { map_name: String, reason: String },

    #[error("interface {interface} does not support XDP mode {mode}")]
    ModeUnsupported { interface: String, mode: String },

    #[error("kernel version {version} does not support required XDP features")]
    KernelVersion { version: String },
}

/// Crate-specific Result alias.
pub type Result<T> = std::result::Result<T, XdpError>;
