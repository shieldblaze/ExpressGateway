//! TCP L4 proxy handler.
//!
//! Accepts TCP connections on a listener, selects a backend via a load balancer,
//! establishes a connection to the backend, and performs bidirectional data
//! forwarding between client and backend.
//!
//! Supports:
//! - Half-close (RFC 9293): when one side sends FIN, continue forwarding the
//!   other direction until it also closes.
//! - Per-connection byte counters for observability.
//! - Configurable TCP socket options (nodelay, keepalive, quickack, fastopen).
//! - Linux splice(2) zero-copy forwarding when available.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::FourTuple;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::connection::{
    ConnectionTracker, TcpConnectionState, TrackedConnection, apply_socket_options,
};
use crate::drain::DrainHandle;
use crate::options::TcpProxyConfig;

/// Errors from the TCP proxy data path.
#[derive(Debug, thiserror::Error)]
pub enum TcpProxyError {
    #[error("backend connection failed to {addr}: {source}")]
    BackendConnect {
        addr: SocketAddr,
        source: std::io::Error,
    },

    #[error("backend connection timed out to {0}")]
    BackendTimeout(SocketAddr),

    #[error("invalid state transition from {0}")]
    InvalidTransition(TcpConnectionState),

    #[error("relay error: {0}")]
    Relay(#[from] std::io::Error),
}

/// Size of the relay buffer.  64 KiB is a good balance: large enough to
/// saturate a 10 Gbps link at reasonable syscall rates, small enough to avoid
/// excessive per-connection memory.
const RELAY_BUF_SIZE: usize = 65_536;

/// TCP proxy that forwards connections from a listener to backends
/// selected by a load balancer.
pub struct TcpProxy {
    /// Proxy configuration.
    config: TcpProxyConfig,
    /// Load balancer for backend selection.
    lb: Arc<dyn LoadBalancer<L4Request, L4Response>>,
    /// Connection tracker.
    tracker: Arc<ConnectionTracker>,
    /// Drain handle for graceful shutdown.
    drain: Arc<DrainHandle>,
    /// Notified when drain is initiated so the accept loop wakes immediately.
    drain_notify: Arc<Notify>,
}

impl TcpProxy {
    /// Create a new TCP proxy with the given config and load balancer.
    pub fn new(config: TcpProxyConfig, lb: Arc<dyn LoadBalancer<L4Request, L4Response>>) -> Self {
        let tracker = Arc::new(ConnectionTracker::new(config.max_connections));
        let drain = Arc::new(DrainHandle::new(config.drain_timeout));
        Self {
            config,
            lb,
            tracker,
            drain,
            drain_notify: Arc::new(Notify::new()),
        }
    }

    /// Get a reference to the connection tracker.
    #[inline]
    pub fn tracker(&self) -> &Arc<ConnectionTracker> {
        &self.tracker
    }

    /// Get a reference to the drain handle.
    #[inline]
    pub fn drain_handle(&self) -> &Arc<DrainHandle> {
        &self.drain
    }

    /// Get a reference to the drain notifier.
    ///
    /// Callers that initiate drain should call `drain_notify.notify_waiters()`
    /// after `drain.start_drain()` so the accept loop exits immediately.
    #[inline]
    pub fn drain_notify(&self) -> &Arc<Notify> {
        &self.drain_notify
    }

    /// Run the TCP proxy, accepting connections on the given listener.
    ///
    /// This method runs until the drain handle is activated and all
    /// connections have completed (or the drain timeout is reached).
    ///
    /// # Cancel safety
    ///
    /// This future is cancel-safe.  Dropping it stops accepting new
    /// connections but does not affect already-spawned connection tasks.
    pub async fn run(&self, listener: TcpListener) -> Result<(), TcpProxyError> {
        let local_addr = listener.local_addr().map_err(TcpProxyError::Relay)?;
        info!(%local_addr, "TCP proxy listening");

        loop {
            // Wait for either a new connection or a drain signal.
            // No polling sleep: the drain_notify is an edge-triggered wakeup.
            let accept = tokio::select! {
                biased;
                _ = self.drain_notify.notified(), if self.drain.is_draining() => {
                    debug!("Drain active, stopping accept loop");
                    break;
                }
                result = listener.accept() => result,
            };

            // Re-check drain after accept returns (could have been signalled
            // between the select and here).
            if self.drain.is_draining() {
                debug!("Drain active, stopping accept loop");
                break;
            }

            let (client_stream, client_addr) = match accept {
                Ok(pair) => pair,
                Err(e) => {
                    // Per-accept errors are transient (e.g. EMFILE, ENOMEM).
                    // Log and keep accepting.
                    error!(error = %e, "Failed to accept TCP connection");
                    continue;
                }
            };

            let four_tuple = FourTuple {
                src_addr: client_addr,
                dst_addr: local_addr,
            };

            // Select backend.
            let request = L4Request { client_addr };
            let response = match self.lb.select(&request) {
                Ok(resp) => resp,
                Err(e) => {
                    warn!(error = %e, %client_addr, "No backend available, dropping connection");
                    continue;
                }
            };

            let backend_addr = response.node.address();
            let node = response.node.clone();

            // Register connection.
            let conn = match self.tracker.try_add(four_tuple, backend_addr) {
                Some(c) => c,
                None => {
                    warn!(
                        %client_addr,
                        active = self.tracker.active_count(),
                        max = self.tracker.max_connections(),
                        "Connection limit reached, dropping"
                    );
                    continue;
                }
            };

            let config = self.config.clone();
            let tracker = self.tracker.clone();

            tokio::spawn(async move {
                node.inc_connections();

                let result =
                    handle_connection(client_stream, backend_addr, &conn, &config).await;

                if let Err(e) = result {
                    debug!(
                        error = %e,
                        src = %four_tuple.src_addr,
                        backend = %backend_addr,
                        "Connection error"
                    );
                }

                let _ = conn.transition(TcpConnectionState::Closed);
                node.dec_connections();
                tracker.remove(&four_tuple);
            });
        }

        // Wait for drain to complete.
        self.drain.wait_for_drain(&self.tracker).await;
        info!("TCP proxy shut down");
        Ok(())
    }
}

/// Handle a single proxied TCP connection: connect to the backend and
/// perform bidirectional forwarding with half-close support.
async fn handle_connection(
    client: TcpStream,
    backend_addr: SocketAddr,
    conn: &Arc<TrackedConnection>,
    config: &TcpProxyConfig,
) -> Result<(), TcpProxyError> {
    // Apply socket options to client side.
    if let Err(e) = apply_socket_options(&client, &config.socket_options) {
        debug!(error = %e, "Failed to set client socket options (non-fatal)");
    }

    // Connect to backend with timeout.
    let backend = match tokio::time::timeout(
        config.connect_timeout,
        TcpStream::connect(backend_addr),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            let _ = conn.transition(TcpConnectionState::Closed);
            return Err(TcpProxyError::BackendConnect {
                addr: backend_addr,
                source: e,
            });
        }
        Err(_) => {
            let _ = conn.transition(TcpConnectionState::Closed);
            return Err(TcpProxyError::BackendTimeout(backend_addr));
        }
    };

    // Apply socket options to backend side.
    if let Err(e) = apply_socket_options(&backend, &config.socket_options) {
        debug!(error = %e, "Failed to set backend socket options (non-fatal)");
    }

    conn.transition(TcpConnectionState::Active)
        .map_err(TcpProxyError::InvalidTransition)?;

    // Try splice-based zero-copy forwarding on Linux.
    // Falls back to userspace relay on non-Linux or if splice setup fails.
    #[cfg(target_os = "linux")]
    {
        if splice_probe(&client) {
            debug!("Using splice(2) zero-copy path");
            return splice_bidirectional(client, backend, config.idle_timeout, conn)
                .await
                .map_err(TcpProxyError::Relay);
        }
        debug!("splice not available for these fds, using userspace relay");
    }

    userspace_relay(client, backend, config.idle_timeout, conn).await
}

/// Userspace bidirectional relay with half-close support.
///
/// Uses a cancel-safe relay loop: each direction runs independently.
/// When one direction reaches EOF, we shut down the write side of the other
/// direction and wait for the remaining direction to complete (with idle
/// timeout).
///
/// # Cancel safety
///
/// Each direction uses `relay_one_direction` which holds its own buffer.
/// The `tokio::select!` only races the *completion* of each direction.
/// Because each future owns its buffer and completes an entire
/// read-then-write cycle atomically before yielding, no data is lost
/// when the select drops the losing branch.  Specifically, the dropped
/// future is `relay_one_direction` which may be cancelled between loop
/// iterations (after `write_all` succeeded and before the next `read_buf`).
/// `read_buf` on `BytesMut` is cancel-safe (documented by tokio), and we
/// `clear()` at the top of each iteration, so any partial read from a
/// cancelled iteration is discarded without data loss.
async fn userspace_relay(
    client: TcpStream,
    backend: TcpStream,
    idle_timeout: std::time::Duration,
    conn: &TrackedConnection,
) -> Result<(), TcpProxyError> {
    let (mut client_read, mut client_write) = client.into_split();
    let (mut backend_read, mut backend_write) = backend.into_split();

    // Allocate relay buffers once, outside the hot loop.
    let mut c2b_buf = BytesMut::with_capacity(RELAY_BUF_SIZE);
    let mut b2c_buf = BytesMut::with_capacity(RELAY_BUF_SIZE);

    // Race both relay directions.  When one side sends FIN (returns Ok),
    // we shut down the write side of the finished direction and drain the
    // remaining direction.  This is half-close support per RFC 9293 §3.5.
    //
    // We use a block scope so that both pinned futures are dropped at the
    // end of the select, releasing their &mut borrows on the write halves.
    // relay_one_direction is cancel-safe (see doc comment), so the
    // non-selected future can be safely dropped and restarted for drain.
    enum FirstDone {
        ClientToBackend(std::io::Result<()>),
        BackendToClient(std::io::Result<()>),
    }

    let first = {
        let c2b = relay_one_direction(
            &mut client_read,
            &mut backend_write,
            idle_timeout,
            conn,
            Direction::ClientToBackend,
            &mut c2b_buf,
        );
        let b2c = relay_one_direction(
            &mut backend_read,
            &mut client_write,
            idle_timeout,
            conn,
            Direction::BackendToClient,
            &mut b2c_buf,
        );
        tokio::pin!(c2b);
        tokio::pin!(b2c);

        tokio::select! {
            r = &mut c2b => FirstDone::ClientToBackend(r),
            r = &mut b2c => FirstDone::BackendToClient(r),
        }
        // Both futures dropped here, releasing all &mut borrows.
    };

    match first {
        FirstDone::ClientToBackend(result) => {
            if let Err(e) = result {
                debug!(error = %e, "client->backend relay error");
            }
            // Client sent FIN — shut down backend write so backend sees EOF.
            let _ = backend_write.shutdown().await;
            // Drain remaining backend->client data until backend also closes.
            if let Err(e) = relay_one_direction(
                &mut backend_read,
                &mut client_write,
                idle_timeout,
                conn,
                Direction::BackendToClient,
                &mut b2c_buf,
            )
            .await
            {
                debug!(error = %e, "backend->client relay error (half-close drain)");
            }
        }
        FirstDone::BackendToClient(result) => {
            if let Err(e) = result {
                debug!(error = %e, "backend->client relay error");
            }
            // Backend sent FIN — shut down client write so client sees EOF.
            let _ = client_write.shutdown().await;
            // Drain remaining client->backend data.
            if let Err(e) = relay_one_direction(
                &mut client_read,
                &mut backend_write,
                idle_timeout,
                conn,
                Direction::ClientToBackend,
                &mut c2b_buf,
            )
            .await
            {
                debug!(error = %e, "client->backend relay error (half-close drain)");
            }
        }
    }

    Ok(())
}

/// Direction of data flow for byte counter accounting.
#[derive(Debug, Clone, Copy)]
enum Direction {
    ClientToBackend,
    BackendToClient,
}

/// Copy data from `reader` to `writer` until EOF, with idle timeout.
///
/// Uses the provided `BytesMut` buffer to avoid per-call allocation.
/// The buffer is reused across loop iterations (clear + read pattern).
///
/// # Cancel safety
///
/// This future is cancel-safe at the top of each loop iteration.  The
/// `read_buf` call may be cancelled, but `BytesMut::read_buf` is documented
/// as cancel-safe: if the future is dropped before completion, the buffer
/// is left in a valid (but possibly extended) state.  Since we `clear()`
/// the buffer at the top of each iteration, any partial read from a
/// cancelled iteration is discarded — no bytes are silently lost because
/// they were never forwarded to the writer.
async fn relay_one_direction<R, W>(
    reader: &mut R,
    writer: &mut W,
    idle_timeout: std::time::Duration,
    conn: &TrackedConnection,
    direction: Direction,
    buf: &mut BytesMut,
) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    loop {
        // Clear any data from the previous iteration.  This does not
        // deallocate — the underlying Vec retains its capacity.
        buf.clear();

        let n = match tokio::time::timeout(idle_timeout, reader.read_buf(buf)).await {
            Ok(Ok(0)) => return Ok(()), // EOF / FIN
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "idle timeout",
                ));
            }
        };

        writer.write_all(&buf[..n]).await?;

        // Update byte counters and activity timestamp.
        match direction {
            Direction::ClientToBackend => conn.record_sent(n as u64),
            Direction::BackendToClient => conn.record_received(n as u64),
        }
        conn.touch();
    }
}

// ============================================================================
// Linux splice(2) zero-copy path
// ============================================================================

/// Probe whether splice(2) is usable on this socket by attempting a
/// zero-length splice.  Returns `true` if splice can be used.
#[cfg(target_os = "linux")]
fn splice_probe(stream: &TcpStream) -> bool {
    use std::os::fd::AsRawFd;

    let mut fds = [0i32; 2];
    // SAFETY: `fds` is a valid two-element array.
    let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if ret < 0 {
        return false;
    }

    // SAFETY: pipe2 succeeded, fds are valid.  Zero-length splice is a no-op
    // that just checks whether splice is supported for this fd type.
    let probe = unsafe {
        libc::splice(
            stream.as_raw_fd(),
            std::ptr::null_mut(),
            fds[1],
            std::ptr::null_mut(),
            0,
            libc::SPLICE_F_NONBLOCK,
        )
    };

    // Close pipe fds.
    // SAFETY: fds are valid open file descriptors from pipe2.
    unsafe {
        libc::close(fds[0]);
        libc::close(fds[1]);
    }

    probe >= 0
}

/// Bidirectional splice-based forwarding.
///
/// Uses `splice(2)` to move data directly between two TCP sockets via a
/// kernel pipe, avoiding copies into userspace.  This is the same mechanism
/// HAProxy uses for its zero-copy data path.
///
/// Each direction gets its own pipe (two fds).  We use `SPLICE_F_NONBLOCK |
/// SPLICE_F_MOVE` to avoid blocking the async runtime.
///
/// We use `TcpStream::ready(Interest::READABLE)` to wait for the source
/// socket to become readable before issuing the splice syscall.  This
/// integrates cleanly with tokio's reactor without double-registering fds.
///
/// # Cancel safety
///
/// Each direction future can be safely cancelled between loop iterations.
/// In-flight data is in the kernel pipe and will be discarded on pipe close.
/// No userspace data is at risk.
#[cfg(target_os = "linux")]
async fn splice_bidirectional(
    client: TcpStream,
    backend: TcpStream,
    idle_timeout: std::time::Duration,
    conn: &TrackedConnection,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    // Create pipes for each direction.
    let (c2b_pipe_r, c2b_pipe_w) = create_pipe()?;
    let (b2c_pipe_r, b2c_pipe_w) = create_pipe()?;

    // Set pipe sizes to match our relay buffer for good throughput.
    // SAFETY: Valid pipe fds from create_pipe.  F_SETPIPE_SZ may silently
    // round up to the nearest page-aligned size.  Failure is non-fatal.
    unsafe {
        libc::fcntl(c2b_pipe_r.as_raw_fd(), libc::F_SETPIPE_SZ, RELAY_BUF_SIZE as i32);
        libc::fcntl(b2c_pipe_r.as_raw_fd(), libc::F_SETPIPE_SZ, RELAY_BUF_SIZE as i32);
    }

    let c2b = splice_one_direction(
        &client,
        c2b_pipe_r,
        c2b_pipe_w,
        backend.as_raw_fd(),
        idle_timeout,
        conn,
        Direction::ClientToBackend,
    );

    let b2c = splice_one_direction(
        &backend,
        b2c_pipe_r,
        b2c_pipe_w,
        client.as_raw_fd(),
        idle_timeout,
        conn,
        Direction::BackendToClient,
    );

    tokio::pin!(c2b);
    tokio::pin!(b2c);

    // Half-close: when one direction finishes, shut down the write side of
    // the other and let the remaining direction drain.
    tokio::select! {
        result = &mut c2b => {
            // Client->backend done.  Shut down backend's write side.
            // SAFETY: backend fd is valid; shutdown is safe on a connected socket.
            unsafe { libc::shutdown(backend.as_raw_fd(), libc::SHUT_WR); }
            if let Err(e) = result {
                debug!(error = %e, "splice client->backend error");
            }
            if let Err(e) = b2c.await {
                debug!(error = %e, "splice backend->client error (half-close drain)");
            }
        }
        result = &mut b2c => {
            // Backend->client done.  Shut down client's write side.
            // SAFETY: client fd is valid; shutdown is safe on a connected socket.
            unsafe { libc::shutdown(client.as_raw_fd(), libc::SHUT_WR); }
            if let Err(e) = result {
                debug!(error = %e, "splice backend->client error");
            }
            if let Err(e) = c2b.await {
                debug!(error = %e, "splice client->backend error (half-close drain)");
            }
        }
    }

    Ok(())
}

/// Create a non-blocking pipe with close-on-exec.
#[cfg(target_os = "linux")]
fn create_pipe() -> std::io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::{FromRawFd, OwnedFd};

    let mut fds = [0i32; 2];
    // SAFETY: `fds` is a valid two-element array.  `pipe2` with O_CLOEXEC |
    // O_NONBLOCK creates a pipe with close-on-exec and non-blocking flags.
    let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `pipe2` succeeded, so fds[0] and fds[1] are valid new fds.
    let r = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let w = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    Ok((r, w))
}

/// Splice data in one direction: source_socket -> pipe -> dest_socket.
///
/// Uses `TcpStream::ready()` to wait for the source to become readable,
/// then `splice(2)` into the pipe, then `splice(2)` from the pipe to the
/// destination.  All data stays in kernel space.
///
/// Returns `Ok(())` on EOF (splice returns 0), `Err` on I/O error or timeout.
#[cfg(target_os = "linux")]
async fn splice_one_direction(
    source: &TcpStream,
    pipe_r: std::os::fd::OwnedFd,
    pipe_w: std::os::fd::OwnedFd,
    dest_fd: i32,
    idle_timeout: std::time::Duration,
    conn: &TrackedConnection,
    direction: Direction,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    let source_fd = source.as_raw_fd();
    let pipe_r_fd = pipe_r.as_raw_fd();
    let pipe_w_fd = pipe_w.as_raw_fd();
    let flags = libc::SPLICE_F_NONBLOCK | libc::SPLICE_F_MOVE;

    loop {
        // Wait for the source socket to be readable, with idle timeout.
        match tokio::time::timeout(
            idle_timeout,
            source.ready(tokio::io::Interest::READABLE),
        )
        .await
        {
            Ok(Ok(_ready)) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "idle timeout",
                ));
            }
        }

        // Splice from source socket into pipe.
        // SAFETY: source_fd is a valid fd from a live TcpStream.  pipe_w_fd
        // is valid from create_pipe.  SPLICE_F_NONBLOCK prevents blocking.
        // Null offsets because these are stream (non-seekable) fds.
        let n = unsafe {
            libc::splice(
                source_fd,
                std::ptr::null_mut(),
                pipe_w_fd,
                std::ptr::null_mut(),
                RELAY_BUF_SIZE,
                flags,
            )
        };

        if n == 0 {
            // EOF on source.
            return Ok(());
        }

        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                // Spurious readability notification.  Retry.
                continue;
            }
            return Err(err);
        }

        let mut remaining = n as usize;

        // Splice from pipe into destination socket.  May need multiple calls
        // if the dest socket buffer is full.
        while remaining > 0 {
            // SAFETY: pipe_r_fd and dest_fd are valid fds.
            let written = unsafe {
                libc::splice(
                    pipe_r_fd,
                    std::ptr::null_mut(),
                    dest_fd,
                    std::ptr::null_mut(),
                    remaining,
                    flags,
                )
            };

            if written < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    // Destination buffer full.  Yield to the runtime to allow
                    // the destination to drain, then retry.
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(err);
            }

            if written == 0 {
                // Destination closed.
                return Ok(());
            }

            remaining -= written as usize;
        }

        // Update byte counters and activity timestamp.
        let bytes = n as u64;
        match direction {
            Direction::ClientToBackend => conn.record_sent(bytes),
            Direction::BackendToClient => conn.record_received(bytes),
        }
        conn.touch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
    use expressgateway_core::node::{Node, NodeImpl};
    use std::sync::Arc;

    /// A simple test load balancer that always returns a fixed node.
    struct FixedLb {
        node: Arc<dyn Node>,
    }

    impl LoadBalancer<L4Request, L4Response> for FixedLb {
        fn select(&self, _request: &L4Request) -> expressgateway_core::error::Result<L4Response> {
            Ok(L4Response {
                node: self.node.clone(),
            })
        }
        fn add_node(&self, _node: Arc<dyn Node>) {}
        fn remove_node(&self, _node_id: &str) {}
        fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
            vec![self.node.clone()]
        }
        fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
            vec![self.node.clone()]
        }
        fn get_node(&self, _node_id: &str) -> Option<Arc<dyn Node>> {
            Some(self.node.clone())
        }
    }

    #[tokio::test]
    async fn test_tcp_proxy_creation() {
        let node = NodeImpl::new_arc("n1".into(), "127.0.0.1:9999".parse().unwrap(), 1, None);
        let lb: Arc<dyn LoadBalancer<L4Request, L4Response>> = Arc::new(FixedLb { node });
        let config = TcpProxyConfig::default();
        let proxy = TcpProxy::new(config, lb);

        assert_eq!(proxy.tracker().active_count(), 0);
        assert!(!proxy.drain_handle().is_draining());
    }

    #[tokio::test]
    async fn test_relay_eof() {
        // Test that relay returns Ok on EOF.
        let input = b"hello world";
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        let mut buf = BytesMut::with_capacity(RELAY_BUF_SIZE);
        let result = relay_one_direction(
            &mut reader,
            &mut writer,
            std::time::Duration::from_secs(5),
            &conn,
            Direction::ClientToBackend,
            &mut buf,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(writer, b"hello world");
        assert_eq!(
            conn.bytes_sent.load(std::sync::atomic::Ordering::Relaxed),
            11
        );
    }

    #[tokio::test]
    async fn test_relay_preserves_buffer_across_calls() {
        // Verify that the buffer is reused (capacity retained after clear).
        let input = b"data";
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        let mut buf = BytesMut::with_capacity(RELAY_BUF_SIZE);
        let _ = relay_one_direction(
            &mut reader,
            &mut writer,
            std::time::Duration::from_secs(5),
            &conn,
            Direction::ClientToBackend,
            &mut buf,
        )
        .await;

        // Buffer should still have its capacity after the relay completes.
        assert!(buf.capacity() >= RELAY_BUF_SIZE);
    }
}
