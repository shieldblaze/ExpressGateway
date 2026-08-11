//! I/O abstraction layer with `io_uring` and epoll fallback. The backend selection is a live
//! capability PROBE, not a version check. Full io_uring ACCEPT/RECV/SEND/SPLICE is not implemented
//! — the enum selects the probe result, not the datapath.
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
#![allow(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod dns;
pub mod http2_pool;
pub mod idle_send;
pub mod pool;
pub mod quic_pool;
pub mod sockopts;

#[cfg(target_os = "linux")]
pub mod ring;

/// I/O backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IoBackend {
    /// Use `io_uring` where available (Linux 5.1+).
    IoUring,
    /// Fall back to epoll (Linux 2.6+) or the platform default.
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

/// Probe for `io_uring` by actually submitting and reaping a `NOP` SQE — a version check would
/// miss `kernel.io_uring_disabled=1`, seccomp filters and permission denials. Anything short of
/// full success falls back to [`IoBackend::Epoll`].
#[must_use]
pub fn detect_backend() -> IoBackend {
    #[cfg(target_os = "linux")]
    {
        match ring::nop_roundtrip() {
            Ok(_) => IoBackend::IoUring,
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "io_uring probe failed; falling back to epoll"
                );
                IoBackend::Epoll
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        tracing::debug!("non-linux target; using epoll backend");
        IoBackend::Epoll
    }
}

/// Records the selected backend and the buffer watermarks used for bidirectional backpressure.
#[derive(Debug, Clone, Copy)]
pub struct Runtime {
    backend: IoBackend,
}

impl Runtime {
    /// Create a new runtime, auto-detecting the best available backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            backend: detect_backend(),
        }
    }

    /// Runtime bound to a specific backend, bypassing detection.
    #[must_use]
    pub const fn with_backend(backend: IoBackend) -> Self {
        Self { backend }
    }

    /// Return the backend this runtime will drive.
    #[must_use]
    pub const fn backend(&self) -> IoBackend {
        self.backend
    }

    /// High watermark: pause reads once the write buffer exceeds this.
    #[must_use]
    pub const fn high_water_mark() -> usize {
        65_536
    }

    /// Low watermark: resume reads once the write buffer drains below this.
    #[must_use]
    pub const fn low_water_mark() -> usize {
        32_768
    }

    /// Bind a TCP listener and apply `opts`.
    pub fn listener_socket(
        &self,
        addr: std::net::SocketAddr,
        cfg: &sockopts::ListenerSockOpts,
    ) -> std::io::Result<std::net::TcpListener> {
        let listener = std::net::TcpListener::bind(addr)?;
        sockopts::apply_listener(&listener, cfg)?;
        Ok(listener)
    }

    /// Connect a TCP socket and apply `opts`.
    pub fn connect_socket(
        &self,
        addr: std::net::SocketAddr,
        cfg: &sockopts::BackendSockOpts,
    ) -> std::io::Result<std::net::TcpStream> {
        let stream = std::net::TcpStream::connect(addr)?;
        sockopts::apply_connected(&stream, cfg)?;
        Ok(stream)
    }

    /// Bind a listener and return it in BLOCKING mode — a tokio caller must `set_nonblocking`
    /// before `from_std`.
    pub fn listen(
        &self,
        addr: std::net::SocketAddr,
        cfg: &sockopts::ListenerSockOpts,
    ) -> std::io::Result<std::net::TcpListener> {
        self.listener_socket(addr, cfg)
    }

    /// Connect and return the stream in BLOCKING mode — a tokio caller must `set_nonblocking`
    /// before `from_std`.
    pub fn connect(
        &self,
        addr: std::net::SocketAddr,
        cfg: &sockopts::BackendSockOpts,
    ) -> std::io::Result<std::net::TcpStream> {
        self.connect_socket(addr, cfg)
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::sockopts::{ListenerSockOpts, apply_listener};
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn backend_names() {
        assert_eq!(IoBackend::IoUring.as_str(), "io_uring");
        assert_eq!(IoBackend::Epoll.as_str(), "epoll");
        assert_eq!(IoBackend::IoUring.to_string(), "io_uring");
        assert_eq!(IoBackend::Epoll.to_string(), "epoll");
    }

    #[test]
    fn runtime_with_backend() {
        let rt = Runtime::with_backend(IoBackend::IoUring);
        assert_eq!(rt.backend(), IoBackend::IoUring);
        let rt = Runtime::with_backend(IoBackend::Epoll);
        assert_eq!(rt.backend(), IoBackend::Epoll);
        let _ = Runtime::default();
        let _ = Runtime::new();
    }

    #[test]
    fn watermarks_present() {
        assert_eq!(Runtime::high_water_mark(), 65_536);
        assert_eq!(Runtime::low_water_mark(), 32_768);
        assert!(Runtime::low_water_mark() < Runtime::high_water_mark());
    }

    #[test]
    fn runtime_listener_and_connect_sockets() {
        use super::sockopts::BackendSockOpts;
        let rt = Runtime::with_backend(IoBackend::Epoll);
        let lcfg = ListenerSockOpts {
            reuseaddr: true,
            nodelay: true,
            keepalive: true,
            ..Default::default()
        };
        let listener = rt
            .listener_socket("127.0.0.1:0".parse().unwrap(), &lcfg)
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let bcfg = BackendSockOpts {
            nodelay: true,
            keepalive: true,
            ..Default::default()
        };
        let stream = rt.connect_socket(addr, &bcfg).unwrap();
        assert_eq!(stream.peer_addr().unwrap(), addr);
    }

    #[test]
    fn detect_backend_returns_real_choice() {
        // The value is kernel-dependent; only the absence of a panic is asserted.
        let b = detect_backend();
        assert!(matches!(b, IoBackend::IoUring | IoBackend::Epoll));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nop_roundtrip_linux() {
        match ring::nop_roundtrip() {
            Ok(res) => assert_eq!(res.user_data, 0xDEAD_BEEF_u64),
            Err(err) => {
                eprintln!("skipping nop_roundtrip_linux: io_uring unavailable ({err})");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn socket_options_listener() {
        use std::os::fd::AsRawFd;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = ListenerSockOpts {
            reuseaddr: true,
            reuseport: true,
            rcvbuf: Some(262_144),
            sndbuf: Some(262_144),
            nodelay: true,
            quickack: true,
            keepalive: true,
            tcp_fastopen: Some(4_096),
            backlog: Some(50_000),
        };
        apply_listener(&listener, &cfg).unwrap();

        let fd = listener.as_raw_fd();
        assert!(getsockopt_bool(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR));
        assert!(getsockopt_bool(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn socket_options_listener_non_linux() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = ListenerSockOpts {
            reuseaddr: true,
            reuseport: false,
            rcvbuf: Some(262_144),
            sndbuf: Some(262_144),
            nodelay: true,
            quickack: false,
            keepalive: true,
            tcp_fastopen: None,
            backlog: None,
        };
        apply_listener(&listener, &cfg).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn getsockopt_bool(fd: i32, level: i32, name: i32) -> bool {
        let mut val: libc::c_int = 0;
        let mut len: libc::socklen_t =
            libc::socklen_t::try_from(core::mem::size_of::<libc::c_int>()).unwrap();
        // SAFETY: fd is a live listener fd owned by the caller, `val` and
        // `len` are local stack variables whose sizes match the kernel's
        // expected `int` / `socklen_t` argument shapes.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                level,
                name,
                std::ptr::addr_of_mut!(val).cast(),
                std::ptr::addr_of_mut!(len),
            )
        };
        assert_eq!(
            rc,
            0,
            "getsockopt failed: {}",
            std::io::Error::last_os_error()
        );
        val != 0
    }
}
