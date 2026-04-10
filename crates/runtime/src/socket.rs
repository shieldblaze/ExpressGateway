//! Socket option configuration for server (listener) and client (backend) sockets.
//!
//! Encapsulates all the TCP/socket tuning knobs specified by the ExpressGateway
//! networking spec and applies them via the `socket2` crate.

use socket2::Socket;
use std::io;

/// Default receive/send buffer size (256 KB).
const DEFAULT_BUF_SIZE: usize = 262_144;

/// Default server backlog depth.
const DEFAULT_BACKLOG: u32 = 50_000;

/// Socket-level and TCP-level options to apply to a socket.
#[derive(Debug, Clone)]
pub struct SocketOptions {
    /// SO_RCVBUF size in bytes.
    pub recv_buf_size: usize,
    /// SO_SNDBUF size in bytes.
    pub send_buf_size: usize,
    /// TCP_NODELAY — disable Nagle's algorithm.
    pub tcp_nodelay: bool,
    /// TCP_QUICKACK — disable delayed ACKs (Linux only).
    pub tcp_quickack: bool,
    /// SO_KEEPALIVE — enable TCP keep-alive probes.
    pub tcp_keepalive: bool,
    /// TCP_FASTOPEN queue length (server) or TCP_FASTOPEN_CONNECT (client).
    /// `None` disables fast-open.
    pub tcp_fastopen: Option<u32>,
    /// SO_REUSEADDR.
    pub so_reuseaddr: bool,
    /// SO_REUSEPORT.
    pub so_reuseport: bool,
    /// Listen backlog depth (only meaningful for listener sockets).
    pub backlog: u32,
}

impl SocketOptions {
    /// Returns the default options for a **server / listener** socket.
    ///
    /// Matches the ExpressGateway spec:
    /// - `SO_REUSEADDR`: true
    /// - `SO_REUSEPORT`: true
    /// - `SO_RCVBUF`: 256 KB
    /// - `SO_SNDBUF`: 256 KB
    /// - `TCP_NODELAY`: true
    /// - `TCP_QUICKACK`: true (Linux only)
    /// - `SO_KEEPALIVE`: true
    /// - `TCP_FASTOPEN`: 256 (queue length)
    /// - `SO_BACKLOG`: 50,000
    pub fn server_defaults() -> Self {
        Self {
            recv_buf_size: DEFAULT_BUF_SIZE,
            send_buf_size: DEFAULT_BUF_SIZE,
            tcp_nodelay: true,
            tcp_quickack: true,
            tcp_keepalive: true,
            tcp_fastopen: Some(256),
            so_reuseaddr: true,
            so_reuseport: true,
            backlog: DEFAULT_BACKLOG,
        }
    }

    /// Returns the default options for a **client / backend** socket.
    ///
    /// Matches the ExpressGateway spec:
    /// - `TCP_NODELAY`: true
    /// - `SO_KEEPALIVE`: true
    /// - `SO_RCVBUF`: 256 KB
    /// - `SO_SNDBUF`: 256 KB
    /// - `TCP_QUICKACK`: true (Linux only)
    /// - `TCP_FASTOPEN_CONNECT`: true
    pub fn client_defaults() -> Self {
        Self {
            recv_buf_size: DEFAULT_BUF_SIZE,
            send_buf_size: DEFAULT_BUF_SIZE,
            tcp_nodelay: true,
            tcp_quickack: true,
            tcp_keepalive: true,
            tcp_fastopen: Some(1), // non-zero signals TCP_FASTOPEN_CONNECT
            so_reuseaddr: false,
            so_reuseport: false,
            backlog: 0, // not applicable for client sockets
        }
    }

    /// Apply socket options appropriate for a **listener** socket.
    ///
    /// This sets buffer sizes, reuse flags, keep-alive, nodelay,
    /// quickack, and TCP fast-open on the provided `socket2::Socket`.
    pub fn apply_to_listener(&self, socket: &Socket) -> io::Result<()> {
        // Buffer sizes.
        socket.set_recv_buffer_size(self.recv_buf_size)?;
        socket.set_send_buffer_size(self.send_buf_size)?;

        // Address / port reuse.
        socket.set_reuse_address(self.so_reuseaddr)?;

        #[cfg(target_os = "linux")]
        socket.set_reuse_port(self.so_reuseport)?;

        // Keep-alive.
        socket.set_keepalive(self.tcp_keepalive)?;

        // Nodelay.
        socket.set_nodelay(self.tcp_nodelay)?;

        // TCP_QUICKACK (Linux only).
        #[cfg(target_os = "linux")]
        if self.tcp_quickack {
            set_tcp_quickack(socket)?;
        }

        // TCP_FASTOPEN (Linux only).
        #[cfg(target_os = "linux")]
        if let Some(queue_len) = self.tcp_fastopen {
            set_tcp_fastopen(socket, queue_len)?;
        }

        Ok(())
    }

    /// Apply socket options appropriate for a **client / backend** socket.
    ///
    /// This sets buffer sizes, keep-alive, nodelay, quickack, and
    /// TCP_FASTOPEN_CONNECT.
    pub fn apply_to_client(&self, socket: &Socket) -> io::Result<()> {
        // Buffer sizes.
        socket.set_recv_buffer_size(self.recv_buf_size)?;
        socket.set_send_buffer_size(self.send_buf_size)?;

        // Keep-alive.
        socket.set_keepalive(self.tcp_keepalive)?;

        // Nodelay.
        socket.set_nodelay(self.tcp_nodelay)?;

        // TCP_QUICKACK (Linux only).
        #[cfg(target_os = "linux")]
        if self.tcp_quickack {
            set_tcp_quickack(socket)?;
        }

        // TCP_FASTOPEN_CONNECT (Linux only).
        #[cfg(target_os = "linux")]
        if self.tcp_fastopen.is_some() {
            set_tcp_fastopen_connect(socket)?;
        }

        Ok(())
    }
}

/// Set `TCP_QUICKACK` on a socket (Linux only).
#[cfg(target_os = "linux")]
fn set_tcp_quickack(socket: &Socket) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let fd = socket.as_raw_fd();
    let val: libc::c_int = 1;
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_QUICKACK,
            &val as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Set `TCP_FASTOPEN` on a listener socket (Linux only).
#[cfg(target_os = "linux")]
fn set_tcp_fastopen(socket: &Socket, queue_len: u32) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let fd = socket.as_raw_fd();
    let val: libc::c_int = queue_len as libc::c_int;
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_FASTOPEN,
            &val as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Set `TCP_FASTOPEN_CONNECT` on a client socket (Linux only).
#[cfg(target_os = "linux")]
fn set_tcp_fastopen_connect(socket: &Socket) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let fd = socket.as_raw_fd();
    // TCP_FASTOPEN_CONNECT = 30 on Linux.
    const TCP_FASTOPEN_CONNECT: libc::c_int = 30;
    let val: libc::c_int = 1;
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            TCP_FASTOPEN_CONNECT,
            &val as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use socket2::{Domain, Protocol, Type};

    #[test]
    fn server_defaults_are_correct() {
        let opts = SocketOptions::server_defaults();
        assert_eq!(opts.recv_buf_size, 262_144);
        assert_eq!(opts.send_buf_size, 262_144);
        assert!(opts.tcp_nodelay);
        assert!(opts.tcp_quickack);
        assert!(opts.tcp_keepalive);
        assert_eq!(opts.tcp_fastopen, Some(256));
        assert!(opts.so_reuseaddr);
        assert!(opts.so_reuseport);
        assert_eq!(opts.backlog, 50_000);
    }

    #[test]
    fn client_defaults_are_correct() {
        let opts = SocketOptions::client_defaults();
        assert_eq!(opts.recv_buf_size, 262_144);
        assert_eq!(opts.send_buf_size, 262_144);
        assert!(opts.tcp_nodelay);
        assert!(opts.tcp_quickack);
        assert!(opts.tcp_keepalive);
        assert_eq!(opts.tcp_fastopen, Some(1));
        assert!(!opts.so_reuseaddr);
        assert!(!opts.so_reuseport);
    }

    #[test]
    fn apply_to_listener_succeeds() {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
            .expect("failed to create socket");
        let opts = SocketOptions::server_defaults();
        opts.apply_to_listener(&socket)
            .expect("apply_to_listener failed");

        // Verify a couple of options actually took effect.
        assert!(socket.nodelay().unwrap());
        assert!(socket.keepalive().unwrap());
        assert!(socket.reuse_address().unwrap());
    }

    #[test]
    fn apply_to_client_succeeds() {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
            .expect("failed to create socket");
        let opts = SocketOptions::client_defaults();
        opts.apply_to_client(&socket)
            .expect("apply_to_client failed");

        assert!(socket.nodelay().unwrap());
        assert!(socket.keepalive().unwrap());
    }

    #[test]
    fn custom_options() {
        let opts = SocketOptions {
            recv_buf_size: 1024,
            send_buf_size: 2048,
            tcp_nodelay: false,
            tcp_quickack: false,
            tcp_keepalive: false,
            tcp_fastopen: None,
            so_reuseaddr: false,
            so_reuseport: false,
            backlog: 128,
        };

        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
            .expect("failed to create socket");
        opts.apply_to_listener(&socket).expect("apply failed");
        assert!(!socket.nodelay().unwrap());
    }
}
