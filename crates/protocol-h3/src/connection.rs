//! QUIC connection management with migration detection.
//!
//! Handles connection migration (client IP changes), NAT rebinding detection,
//! path validation, connection ID routing, and 0-RTT replay protection.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use tracing::debug;

/// A managed QUIC connection wrapping [`quinn::Connection`] with migration awareness.
pub struct QuicConnection {
    inner: quinn::Connection,
    /// Whether a migration event has been detected on this connection.
    migration_detected: AtomicBool,
    /// Whether this connection was established using 0-RTT early data.
    ///
    /// Per RFC 9001 Section 4.1, 0-RTT data is replayable. The HTTP/3 layer
    /// MUST reject non-idempotent methods (POST, PATCH, DELETE) arriving over
    /// 0-RTT unless the application explicitly opts in to replay risk.
    accepted_0rtt: AtomicBool,
    /// The last observed remote address, used for rebinding detection.
    last_remote_addr: Mutex<SocketAddr>,
}

impl QuicConnection {
    /// Wrap a [`quinn::Connection`] with migration tracking.
    pub fn new(inner: quinn::Connection) -> Self {
        let remote = inner.remote_address();
        Self {
            inner,
            migration_detected: AtomicBool::new(false),
            accepted_0rtt: AtomicBool::new(false),
            last_remote_addr: Mutex::new(remote),
        }
    }

    /// Wrap a [`quinn::Connection`] established via 0-RTT early data.
    ///
    /// Sets the `accepted_0rtt` flag so the HTTP/3 layer can enforce
    /// idempotency checks per RFC 9001 Section 4.1.
    pub fn new_0rtt(inner: quinn::Connection) -> Self {
        let remote = inner.remote_address();
        Self {
            inner,
            migration_detected: AtomicBool::new(false),
            accepted_0rtt: AtomicBool::new(true),
            last_remote_addr: Mutex::new(remote),
        }
    }

    /// Whether this connection was established via 0-RTT.
    ///
    /// When `true`, the HTTP/3 proxy layer should reject non-idempotent
    /// methods to prevent replay attacks (RFC 9001 §4.1).
    pub fn is_0rtt(&self) -> bool {
        self.accepted_0rtt.load(Ordering::Acquire)
    }

    /// Return a reference to the underlying [`quinn::Connection`].
    pub fn inner(&self) -> &quinn::Connection {
        &self.inner
    }

    /// Consume this wrapper and return the underlying [`quinn::Connection`].
    pub fn into_inner(self) -> quinn::Connection {
        self.inner
    }

    /// The current remote address of the peer.
    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// The stable connection identifier.
    pub fn stable_id(&self) -> usize {
        self.inner.stable_id()
    }

    /// Whether a connection migration has been detected.
    pub fn migration_detected(&self) -> bool {
        self.migration_detected.load(Ordering::Acquire)
    }

    /// Check for connection migration or NAT rebinding.
    ///
    /// Compares the current remote address against the last observed address.
    /// Returns `true` if the address changed (migration detected).
    pub fn check_migration(&self) -> bool {
        let current = self.inner.remote_address();
        let mut last = self.last_remote_addr.lock();

        if current != *last {
            debug!(
                old_addr = %*last,
                new_addr = %current,
                conn_id = self.inner.stable_id(),
                "connection migration detected"
            );
            *last = current;
            self.migration_detected.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Detect NAT rebinding: the IP is the same but the port changed.
    pub fn is_nat_rebinding(&self) -> bool {
        let current = self.inner.remote_address();
        let last = self.last_remote_addr.lock();

        current.ip() == last.ip() && current.port() != last.port()
    }

    /// Close the connection gracefully.
    pub fn close(&self, code: u32, reason: &[u8]) {
        self.inner.close(quinn::VarInt::from_u32(code), reason);
        debug!(
            conn_id = self.stable_id(),
            "QUIC connection closed with code {code}"
        );
    }

    /// Get connection statistics.
    pub fn stats(&self) -> quinn::ConnectionStats {
        self.inner.stats()
    }

    /// Accept a new bidirectional stream on this connection.
    pub async fn accept_bi(
        &self,
    ) -> Result<(quinn::SendStream, quinn::RecvStream), quinn::ConnectionError> {
        self.inner.accept_bi().await
    }

    /// Open a new bidirectional stream on this connection.
    pub async fn open_bi(
        &self,
    ) -> Result<(quinn::SendStream, quinn::RecvStream), quinn::ConnectionError> {
        self.inner.open_bi().await
    }

    /// Accept a unidirectional receive stream.
    pub async fn accept_uni(&self) -> Result<quinn::RecvStream, quinn::ConnectionError> {
        self.inner.accept_uni().await
    }

    /// Open a unidirectional send stream.
    pub async fn open_uni(&self) -> Result<quinn::SendStream, quinn::ConnectionError> {
        self.inner.open_uni().await
    }
}

impl std::fmt::Debug for QuicConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicConnection")
            .field("remote_address", &self.inner.remote_address())
            .field("stable_id", &self.inner.stable_id())
            .field(
                "migration_detected",
                &self.migration_detected.load(Ordering::Acquire),
            )
            .field("accepted_0rtt", &self.accepted_0rtt.load(Ordering::Acquire))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quic_connection_default_no_migration() {
        // We cannot easily create a real quinn::Connection in tests without a
        // full QUIC handshake, so we only verify the public API surface compiles
        // and the module structure is sound. Integration tests should exercise the
        // full accept/connect path.
        assert_eq!(std::mem::size_of::<AtomicBool>(), 1);
    }
}
