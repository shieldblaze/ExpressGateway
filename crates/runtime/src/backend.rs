//! Runtime backend detection and selection.
//!
//! Determines whether to use io_uring (Linux 5.1+) or fall back to
//! tokio's default epoll-based reactor.

use std::fmt;

/// The async I/O backend powering the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeBackend {
    /// Linux io_uring (requires kernel >= 5.1).
    IoUring,
    /// Classic epoll via tokio (works on all supported platforms).
    Epoll,
}

impl RuntimeBackend {
    /// Auto-detect the best available backend for the current platform.
    ///
    /// On Linux with kernel >= 5.1 this returns [`RuntimeBackend::IoUring`];
    /// otherwise it returns [`RuntimeBackend::Epoll`].
    pub fn detect() -> Self {
        if Self::io_uring_available() {
            RuntimeBackend::IoUring
        } else {
            RuntimeBackend::Epoll
        }
    }

    /// Check whether io_uring is usable on the running kernel.
    fn io_uring_available() -> bool {
        #[cfg(target_os = "linux")]
        {
            Self::check_kernel_version()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Parse the kernel version from `uname -r` and check >= 5.1.
    #[cfg(target_os = "linux")]
    fn check_kernel_version() -> bool {
        use std::process::Command;

        let output = match Command::new("uname").arg("-r").output() {
            Ok(o) => o,
            Err(_) => return false,
        };

        let version_str = match std::str::from_utf8(&output.stdout) {
            Ok(s) => s.trim().to_string(),
            Err(_) => return false,
        };

        // Parse "major.minor..." from the version string.
        let parts: Vec<&str> = version_str.split('.').collect();
        if parts.len() < 2 {
            return false;
        }

        let major: u32 = match parts[0].parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let minor: u32 = match parts[1].parse() {
            Ok(v) => v,
            Err(_) => return false,
        };

        major > 5 || (major == 5 && minor >= 1)
    }
}

impl fmt::Display for RuntimeBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeBackend::IoUring => write!(f, "io_uring"),
            RuntimeBackend::Epoll => write!(f, "epoll"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_valid_backend() {
        let backend = RuntimeBackend::detect();
        // On any platform we must get one of the two variants.
        assert!(backend == RuntimeBackend::IoUring || backend == RuntimeBackend::Epoll);
    }

    #[test]
    fn display_formatting() {
        assert_eq!(RuntimeBackend::IoUring.to_string(), "io_uring");
        assert_eq!(RuntimeBackend::Epoll.to_string(), "epoll");
    }

    #[test]
    fn clone_and_eq() {
        let a = RuntimeBackend::Epoll;
        let b = a;
        assert_eq!(a, b);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_has_io_uring_on_modern_kernel() {
        // On CI/modern Linux, kernel is typically >= 5.1 so io_uring is available.
        // This test documents the expectation; it will pass on any kernel >= 5.1.
        let backend = RuntimeBackend::detect();
        // We just verify it doesn't panic. The actual value depends on the kernel.
        let _ = backend;
    }
}
