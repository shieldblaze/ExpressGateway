//! Runtime backend detection and selection.
//!
//! Determines whether to use io_uring (Linux 5.11+) or fall back to
//! tokio's default epoll-based reactor.  Detection uses `libc::uname(2)`
//! directly to avoid spawning a child process on the hot startup path.

use std::fmt;

/// The async I/O backend powering the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeBackend {
    /// Linux io_uring (requires kernel >= 5.11 for SQPOLL + multi-shot).
    IoUring,
    /// Classic epoll via tokio (works on all supported platforms).
    Epoll,
}

impl RuntimeBackend {
    /// Auto-detect the best available backend for the current platform.
    ///
    /// On Linux with kernel >= 5.11 this returns [`RuntimeBackend::IoUring`];
    /// otherwise it returns [`RuntimeBackend::Epoll`].
    pub fn detect() -> Self {
        if Self::io_uring_available() {
            RuntimeBackend::IoUring
        } else {
            RuntimeBackend::Epoll
        }
    }

    /// Resolve a backend from the config string (`"auto"`, `"io_uring"`,
    /// `"epoll"`).  Returns `None` for unrecognised values.
    pub fn from_config(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::detect()),
            "io_uring" => Some(Self::IoUring),
            "epoll" => Some(Self::Epoll),
            _ => None,
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

    /// Parse the kernel version from `uname(2)` and check >= 5.11.
    ///
    /// We require 5.11 (not 5.1) because that is the minimum for SQPOLL
    /// mode and multi-shot accept/recv which the io_uring backend relies on.
    #[cfg(target_os = "linux")]
    fn check_kernel_version() -> bool {
        // SAFETY: `libc::utsname` is a POD struct of fixed-size `c_char` arrays.
        // Zero-initialisation produces a valid (if meaningless) value -- every
        // field is a NUL-terminated empty string, which is a valid state.
        let mut info: libc::utsname = unsafe { std::mem::zeroed() };

        // SAFETY: `libc::uname` writes into the caller-provided `libc::utsname`
        // through a valid, mutable, aligned pointer.  The struct lives on the
        // stack for the duration of the call.  `uname(2)` does not retain the
        // pointer.  On success the kernel NUL-terminates every field.
        let ret = unsafe { libc::uname(&mut info) };
        if ret != 0 {
            return false;
        }

        // SAFETY: On success, `uname(2)` guarantees `release` is a
        // NUL-terminated C string within the fixed-size array.  `as_ptr()`
        // yields a pointer into the stack-local `info`, which is alive here.
        let release = unsafe {
            std::ffi::CStr::from_ptr(info.release.as_ptr())
        };

        let version_str = match release.to_str() {
            Ok(s) => s,
            Err(_) => return false,
        };

        // Parse "major.minor..." without allocating a Vec.
        let mut parts = version_str.splitn(3, '.');
        let major_str = match parts.next() {
            Some(s) => s,
            None => return false,
        };
        let minor_str = match parts.next() {
            Some(s) => s,
            None => return false,
        };

        let major: u32 = match major_str.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let minor: u32 = match minor_str.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };

        major > 5 || (major == 5 && minor >= 11)
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

    #[test]
    fn from_config_auto() {
        let b = RuntimeBackend::from_config("auto").unwrap();
        assert!(b == RuntimeBackend::IoUring || b == RuntimeBackend::Epoll);
    }

    #[test]
    fn from_config_explicit() {
        assert_eq!(
            RuntimeBackend::from_config("epoll"),
            Some(RuntimeBackend::Epoll)
        );
        assert_eq!(
            RuntimeBackend::from_config("io_uring"),
            Some(RuntimeBackend::IoUring)
        );
    }

    #[test]
    fn from_config_invalid() {
        assert_eq!(RuntimeBackend::from_config("magic"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_detection_does_not_panic() {
        let backend = RuntimeBackend::detect();
        let _ = backend;
    }
}
