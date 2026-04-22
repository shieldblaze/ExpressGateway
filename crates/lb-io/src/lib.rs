//! I/O abstraction layer with `io_uring` and epoll fallback.
//!
//! Provides an `IoBackend` enum for selecting the kernel I/O mechanism and
//! a detection function for runtime capability probing.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// I/O backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoBackend {
    /// Use `io_uring` where available (Linux 5.1+).
    IoUring,
    /// Fall back to epoll (Linux 2.6+).
    Epoll,
}

impl IoBackend {
    /// Return the name of the backend as a static string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IoUring => "io_uring",
            Self::Epoll => "epoll",
        }
    }
}

impl std::fmt::Display for IoBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors from I/O backend operations.
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    /// The requested I/O backend is not available on this platform.
    #[error("io backend not available: {0}")]
    NotAvailable(String),
}

/// Detect the best available I/O backend for the current platform.
///
/// Currently always returns `Epoll` since `io_uring` requires runtime probing
/// of kernel capabilities that is not yet implemented.
#[must_use]
pub const fn detect_backend() -> IoBackend {
    IoBackend::Epoll
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_epoll() {
        assert_eq!(detect_backend(), IoBackend::Epoll);
    }

    #[test]
    fn display_names() {
        assert_eq!(IoBackend::IoUring.as_str(), "io_uring");
        assert_eq!(IoBackend::Epoll.as_str(), "epoll");
        assert_eq!(IoBackend::Epoll.to_string(), "epoll");
    }
}
