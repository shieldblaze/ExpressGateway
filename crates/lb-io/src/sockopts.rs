//! Socket option helpers matching PROMPT.md §7 "Socket Options".
//!
//! Three typed configuration structs carry the desired options for listener
//! sockets, backend (client) sockets, and UDP sockets. The corresponding
//! `apply_*` functions push those options into the kernel using [`socket2`]
//! for the portable subset and raw `libc::setsockopt` for Linux-only knobs
//! (`TCP_QUICKACK`, `TCP_FASTOPEN`, `TCP_FASTOPEN_CONNECT`, `UDP_GRO`).
//!
//! Wiring these helpers into protocol-specific listeners is deferred to
//! Pillar 1b; the helpers here stand alone and are covered by unit tests.

use std::io;
use std::net::{TcpListener, TcpStream, UdpSocket};

use socket2::SockRef;

/// Options for listener (server) sockets.
///
/// Field defaults are zero / `false` / `None`; callers should explicitly
/// populate the fields they want the kernel to see.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // each bool is a distinct kernel knob
pub struct ListenerSockOpts {
    /// `SO_REUSEADDR`.
    pub reuseaddr: bool,
    /// `SO_REUSEPORT` (Linux / BSD).
    pub reuseport: bool,
    /// `SO_RCVBUF` in bytes, if set.
    pub rcvbuf: Option<usize>,
    /// `SO_SNDBUF` in bytes, if set.
    pub sndbuf: Option<usize>,
    /// `TCP_NODELAY` — disable Nagle's algorithm.
    pub nodelay: bool,
    /// `TCP_QUICKACK` — Linux only.
    pub quickack: bool,
    /// `SO_KEEPALIVE`.
    pub keepalive: bool,
    /// `TCP_FASTOPEN` queue length, if set. Linux only.
    pub tcp_fastopen: Option<u32>,
    /// Listen backlog passed to `listen(2)`, if set. Linux only; on other
    /// platforms this field is accepted but ignored.
    pub backlog: Option<i32>,
}

/// Options for backend (client) sockets after `connect(2)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // each bool is a distinct kernel knob
pub struct BackendSockOpts {
    /// `TCP_NODELAY`.
    pub nodelay: bool,
    /// `SO_KEEPALIVE`.
    pub keepalive: bool,
    /// `SO_RCVBUF` in bytes, if set.
    pub rcvbuf: Option<usize>,
    /// `SO_SNDBUF` in bytes, if set.
    pub sndbuf: Option<usize>,
    /// `TCP_QUICKACK` — Linux only.
    pub quickack: bool,
    /// `TCP_FASTOPEN_CONNECT` — Linux only; enables client-side TFO.
    pub tcp_fastopen_connect: bool,
}

/// Options for UDP sockets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UdpSockOpts {
    /// `SO_REUSEPORT` (Linux / BSD).
    pub reuseport: bool,
    /// `SO_RCVBUF` in bytes, if set.
    pub rcvbuf: Option<usize>,
    /// `SO_SNDBUF` in bytes, if set.
    pub sndbuf: Option<usize>,
    /// `UDP_GRO` — Linux 5.0+. Ignored elsewhere.
    pub udp_gro: bool,
}

/// Apply [`ListenerSockOpts`] to an already-bound [`TcpListener`].
///
/// # Errors
/// Returns any `io::Error` reported by `setsockopt` (or `listen(2)` when
/// `backlog` is set). The first failure aborts the remaining options.
pub fn apply_listener(socket: &TcpListener, cfg: &ListenerSockOpts) -> io::Result<()> {
    let sock = SockRef::from(socket);
    if cfg.reuseaddr {
        sock.set_reuse_address(true)?;
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd"))]
    if cfg.reuseport {
        sock.set_reuse_port(true)?;
    }
    if let Some(sz) = cfg.rcvbuf {
        sock.set_recv_buffer_size(sz)?;
    }
    if let Some(sz) = cfg.sndbuf {
        sock.set_send_buffer_size(sz)?;
    }
    if cfg.nodelay {
        sock.set_nodelay(true)?;
    }
    if cfg.keepalive {
        sock.set_keepalive(true)?;
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let fd = socket.as_raw_fd();
        if cfg.quickack {
            set_int(fd, libc::IPPROTO_TCP, libc::TCP_QUICKACK, 1)?;
        }
        if let Some(qlen) = cfg.tcp_fastopen {
            let qlen_i = i32::try_from(qlen).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "TCP_FASTOPEN queue length exceeds i32::MAX",
                )
            })?;
            set_int(fd, libc::IPPROTO_TCP, libc::TCP_FASTOPEN, qlen_i)?;
        }
        if let Some(backlog) = cfg.backlog {
            // SAFETY: `socket` is a live, bound TCP listener fd that we
            // borrow immutably; `listen(2)` takes an `int` backlog and has
            // no memory effects. A non-zero return is translated into an
            // io::Error via `last_os_error`.
            let rc = unsafe { libc::listen(fd, backlog) };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }

    // Silence unused-warning on non-linux.
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cfg.quickack;
        let _ = cfg.tcp_fastopen;
        let _ = cfg.backlog;
    }

    Ok(())
}

/// Apply [`BackendSockOpts`] to a connected [`TcpStream`].
///
/// # Errors
/// Returns any `io::Error` reported by `setsockopt`. The first failure
/// aborts the remaining options.
pub fn apply_connected(socket: &TcpStream, cfg: &BackendSockOpts) -> io::Result<()> {
    let sock = SockRef::from(socket);
    if cfg.nodelay {
        sock.set_nodelay(true)?;
    }
    if cfg.keepalive {
        sock.set_keepalive(true)?;
    }
    if let Some(sz) = cfg.rcvbuf {
        sock.set_recv_buffer_size(sz)?;
    }
    if let Some(sz) = cfg.sndbuf {
        sock.set_send_buffer_size(sz)?;
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let fd = socket.as_raw_fd();
        if cfg.quickack {
            set_int(fd, libc::IPPROTO_TCP, libc::TCP_QUICKACK, 1)?;
        }
        if cfg.tcp_fastopen_connect {
            // TCP_FASTOPEN_CONNECT = 30. Not exported by all `libc` versions
            // we may pick up transitively, so reference by numeric value.
            const TCP_FASTOPEN_CONNECT: libc::c_int = 30;
            set_int(fd, libc::IPPROTO_TCP, TCP_FASTOPEN_CONNECT, 1)?;
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = cfg.quickack;
        let _ = cfg.tcp_fastopen_connect;
    }

    Ok(())
}

/// Apply [`UdpSockOpts`] to a bound [`UdpSocket`].
///
/// # Errors
/// Returns any `io::Error` reported by `setsockopt`. The first failure
/// aborts the remaining options.
pub fn apply_udp(socket: &UdpSocket, cfg: &UdpSockOpts) -> io::Result<()> {
    let sock = SockRef::from(socket);
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd"))]
    if cfg.reuseport {
        sock.set_reuse_port(true)?;
    }
    if let Some(sz) = cfg.rcvbuf {
        sock.set_recv_buffer_size(sz)?;
    }
    if let Some(sz) = cfg.sndbuf {
        sock.set_send_buffer_size(sz)?;
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let fd = socket.as_raw_fd();
        if cfg.udp_gro {
            // UDP_GRO = 104, see <netinet/udp.h> on Linux 5.0+.
            const UDP_GRO: libc::c_int = 104;
            set_int(fd, libc::IPPROTO_UDP, UDP_GRO, 1)?;
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = cfg.reuseport;
        let _ = cfg.udp_gro;
    }

    Ok(())
}

/// Linux-only helper: set an integer-valued socket option.
#[cfg(target_os = "linux")]
fn set_int(
    fd: libc::c_int,
    level: libc::c_int,
    name: libc::c_int,
    value: libc::c_int,
) -> io::Result<()> {
    let len = libc::socklen_t::try_from(core::mem::size_of::<libc::c_int>())
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "c_int size exceeds socklen_t"))?;
    // SAFETY: `fd` is a live socket file descriptor borrowed by the caller.
    // `&value` points to a local `c_int` on the stack; its size matches the
    // `socklen_t` we pass. `setsockopt` does not retain the pointer beyond
    // the duration of this call.
    let rc = unsafe { libc::setsockopt(fd, level, name, std::ptr::addr_of!(value).cast(), len) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_listener_defaults_is_noop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        apply_listener(&listener, &ListenerSockOpts::default()).unwrap();
    }

    #[test]
    fn apply_backend_minimal() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let cfg = BackendSockOpts {
            nodelay: true,
            keepalive: true,
            rcvbuf: Some(65_536),
            sndbuf: Some(65_536),
            ..Default::default()
        };
        apply_connected(&client, &cfg).unwrap();
    }

    #[test]
    fn apply_udp_minimal() {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let cfg = UdpSockOpts {
            reuseport: false,
            rcvbuf: Some(65_536),
            sndbuf: Some(65_536),
            udp_gro: false,
        };
        apply_udp(&sock, &cfg).unwrap();
    }
}
