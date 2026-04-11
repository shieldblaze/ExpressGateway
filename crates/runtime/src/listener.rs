//! TCP listener builder with full socket-option tuning.
//!
//! [`TcpListenerBuilder`] creates `tokio::net::TcpListener` instances with all
//! the socket options required by the ExpressGateway spec (buffer sizes, reuse
//! flags, fast-open, quickack, etc.).

use std::net::SocketAddr;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;

use crate::socket::SocketOptions;

/// Builder for a properly-configured TCP listener.
///
/// # Example
///
/// ```no_run
/// # fn example() -> std::io::Result<()> {
/// use expressgateway_runtime::listener::TcpListenerBuilder;
///
/// let listener = TcpListenerBuilder::new("0.0.0.0:8080".parse().unwrap())
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct TcpListenerBuilder {
    addr: SocketAddr,
    options: SocketOptions,
}

impl TcpListenerBuilder {
    /// Create a new builder targeting `addr` with server-default socket options.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            options: SocketOptions::server_defaults(),
        }
    }

    /// Override the socket options.
    pub fn with_options(mut self, options: SocketOptions) -> Self {
        self.options = options;
        self
    }

    /// Set the receive buffer size.
    pub fn recv_buf_size(mut self, size: usize) -> Self {
        self.options.recv_buf_size = size;
        self
    }

    /// Set the send buffer size.
    pub fn send_buf_size(mut self, size: usize) -> Self {
        self.options.send_buf_size = size;
        self
    }

    /// Set the TCP fast-open queue length. `None` disables fast-open.
    pub fn tcp_fastopen(mut self, queue_len: Option<u32>) -> Self {
        self.options.tcp_fastopen = queue_len;
        self
    }

    /// Set the listen backlog depth.
    pub fn backlog(mut self, backlog: u32) -> Self {
        self.options.backlog = backlog;
        self
    }

    /// Build and return a fully-configured `tokio::net::TcpListener`.
    ///
    /// This creates a raw socket, applies all configured options, binds,
    /// listens, then converts to a non-blocking tokio listener.
    ///
    /// This method is synchronous despite returning `io::Result` -- all
    /// operations are non-blocking syscalls (`socket`, `setsockopt`, `bind`,
    /// `listen`).  It is kept non-async intentionally to avoid an unnecessary
    /// state-machine transformation and to make it safe to call from both
    /// sync and async contexts.
    pub fn build(self) -> std::io::Result<TcpListener> {
        let domain = if self.addr.is_ipv4() {
            Domain::IPV4
        } else {
            Domain::IPV6
        };

        let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

        // Apply all listener socket options.
        self.options.apply_to_listener(&socket)?;

        // Bind.
        let addr: socket2::SockAddr = self.addr.into();
        socket.bind(&addr)?;

        // Listen with the configured backlog.  Clamp to i32::MAX to prevent
        // truncation on the `as i32` cast.
        let backlog = self.options.backlog.min(i32::MAX as u32) as i32;
        socket.listen(backlog)?;

        // Convert to non-blocking for tokio.
        socket.set_nonblocking(true)?;
        let std_listener: std::net::TcpListener = socket.into();
        TcpListener::from_std(std_listener)
    }
}

/// Create a client (backend) `socket2::Socket` with the spec's default options applied.
///
/// The caller is responsible for connecting the socket.
pub fn new_client_socket(addr: &SocketAddr) -> std::io::Result<Socket> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };

    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    let opts = SocketOptions::client_defaults();
    opts.apply_to_client(&socket)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn listener_binds_and_accepts() {
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
        let listener = TcpListenerBuilder::new(addr)
            .build()
            .expect("failed to build listener");

        let local_addr = listener.local_addr().expect("no local addr");

        // Spawn a client that connects and sends data.
        let client_task = tokio::spawn(async move {
            let mut stream = TcpStream::connect(local_addr)
                .await
                .expect("failed to connect");
            stream.write_all(b"hello").await.expect("write failed");
            stream.shutdown().await.expect("shutdown failed");
        });

        // Accept the connection.
        let (mut stream, _peer) = listener.accept().await.expect("accept failed");

        // Read the data.
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 16];
        let n = stream.read(&mut buf).await.expect("read failed");
        assert_eq!(&buf[..n], b"hello");

        client_task.await.expect("client task failed");
    }

    #[tokio::test]
    async fn listener_with_custom_backlog() {
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
        let listener = TcpListenerBuilder::new(addr)
            .backlog(128)
            .build()
            .expect("failed to build listener with custom backlog");

        // Just verify it bound successfully.
        let local = listener.local_addr().unwrap();
        assert_ne!(local.port(), 0);
    }

    #[tokio::test]
    async fn listener_ipv6() {
        let addr: SocketAddr = "[::1]:0".parse().unwrap();
        let listener = TcpListenerBuilder::new(addr)
            .build()
            .expect("failed to build IPv6 listener");

        let local = listener.local_addr().unwrap();
        assert!(local.is_ipv6());
    }

    #[test]
    fn new_client_socket_ipv4() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let socket = new_client_socket(&addr).expect("failed to create client socket");
        // Verify it's non-blocking.
        assert!(socket.nonblocking().unwrap());
    }

    #[test]
    fn new_client_socket_ipv6() {
        let addr: SocketAddr = "[::1]:8080".parse().unwrap();
        let socket = new_client_socket(&addr).expect("failed to create IPv6 client socket");
        assert!(socket.nonblocking().unwrap());
    }
}
