//! QUIC transport layer backed by [`quinn`] 0.11 over rustls (`ring` backend).
//!
//! This crate exposes two layers:
//!
//! 1. The typed data model — [`QuicDatagram`] and [`QuicStream`] — that the
//!    rest of the gateway (L7 pipeline, HTTP/3 codec, observability) passes
//!    around.
//! 2. A real UDP + TLS transport, hosted inside [`QuicEndpoint`], with
//!    [`roundtrip_datagram`] and [`roundtrip_stream`] exercising quinn's
//!    unreliable-datagram and unidirectional-stream APIs end-to-end. These
//!    are the functions that drive the manifest-locked
//!    `tests/quic_native.rs` coverage.
//!
//! The free functions [`forward_datagram`] and [`forward_stream`] remain as
//! thin synchronous validators: they do **no** network I/O. They guard the
//! typed model against empty payloads and return a clone. Historically
//! they were the crate's entire contract; today they are back-compat
//! helpers kept alive because the crate's own in-module tests and upstream
//! callers expect a zero-I/O validation surface. All real transport work
//! flows through [`QuicEndpoint`].
//!
//! Pillar 3b will add: stateless retry with token validation, connection
//! migration, 0-RTT replay protection, CID-routed pools, Alt-Svc injection,
//! and integration into the root binary (`crates/lb/src/main.rs`).
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

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;

use rustls::RootCertStore;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// Errors from the QUIC layer.
#[derive(Debug, thiserror::Error)]
pub enum QuicError {
    /// Datagram payload is empty.
    #[error("empty datagram payload")]
    EmptyPayload,

    /// Stream data is empty.
    #[error("empty stream data")]
    EmptyStreamData,

    /// Invalid connection ID.
    #[error("invalid connection id: {0}")]
    InvalidConnectionId(u64),

    /// I/O error binding or driving the underlying UDP socket / endpoint.
    #[error("quic i/o error: {0}")]
    Io(#[from] io::Error),

    /// TLS / crypto configuration error.
    #[error("quic tls error: {0}")]
    Tls(#[from] rustls::Error),

    /// No incoming connection was accepted before the endpoint closed.
    #[error("no incoming connection")]
    NoIncoming,

    /// Error while establishing the client side of the connection.
    #[error("quic connect error: {0}")]
    Connect(#[from] quinn::ConnectError),

    /// Error while driving an established quinn connection.
    #[error("quic connection error: {0}")]
    Connection(#[from] quinn::ConnectionError),

    /// Error sending an unreliable datagram.
    #[error("quic send-datagram error: {0}")]
    SendDatagram(#[from] quinn::SendDatagramError),

    /// Error writing to a quinn send stream.
    #[error("quic stream write error: {0}")]
    StreamWrite(#[from] quinn::WriteError),

    /// Error reading a quinn recv stream to end.
    #[error("quic stream read error: {0}")]
    StreamRead(#[from] quinn::ReadToEndError),

    /// Internal task-join failure. Should not occur in practice.
    #[error("quic internal error: {0}")]
    Internal(String),
}

/// QUIC datagram — a connection-scoped, unreliable, bounded payload.
#[derive(Debug, Clone)]
pub struct QuicDatagram {
    /// Connection ID this datagram belongs to.
    pub connection_id: u64,
    /// Raw payload bytes.
    pub data: Vec<u8>,
}

/// QUIC stream frame — a unidirectional stream slice with a FIN marker.
#[derive(Debug, Clone)]
pub struct QuicStream {
    /// Stream ID within the connection.
    pub stream_id: u64,
    /// Stream payload bytes.
    pub data: Vec<u8>,
    /// Whether this is the final frame on the stream.
    pub fin: bool,
}

/// Back-compat synchronous datagram validator. Does **no** network I/O.
/// Use [`roundtrip_datagram`] for real transport.
///
/// # Errors
///
/// Returns [`QuicError::EmptyPayload`] if the datagram has no data.
pub fn forward_datagram(dg: &QuicDatagram) -> Result<QuicDatagram, QuicError> {
    if dg.data.is_empty() {
        return Err(QuicError::EmptyPayload);
    }
    Ok(dg.clone())
}

/// Back-compat synchronous stream-frame validator. Does **no** network I/O.
/// Use [`roundtrip_stream`] for real transport.
///
/// # Errors
///
/// Returns [`QuicError::EmptyStreamData`] if the stream has no data.
pub fn forward_stream(stream: &QuicStream) -> Result<QuicStream, QuicError> {
    if stream.data.is_empty() {
        return Err(QuicError::EmptyStreamData);
    }
    Ok(stream.clone())
}

/// A quinn-backed QUIC endpoint bound to 127.0.0.1. Holds either side
/// (server or client) of a real QUIC connection.
#[derive(Debug, Clone)]
pub struct QuicEndpoint {
    inner: quinn::Endpoint,
    local_addr: SocketAddr,
}

impl QuicEndpoint {
    /// Build a server endpoint bound to an ephemeral `127.0.0.1` port,
    /// using the provided DER-encoded certificate chain entry and PKCS#8
    /// DER private key. The actual bound port is available via
    /// [`local_addr`](Self::local_addr).
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if the UDP socket fails to bind, or if the
    /// TLS configuration is rejected (wrapped as [`io::ErrorKind::Other`]).
    pub fn server_on_loopback(cert_der: Vec<u8>, key_der: Vec<u8>) -> io::Result<Self> {
        let cert = CertificateDer::from(cert_der);
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));
        let server_cfg =
            quinn::ServerConfig::with_single_cert(vec![cert], key).map_err(io::Error::other)?;
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
        let endpoint = quinn::Endpoint::server(server_cfg, addr)?;
        let local_addr = endpoint.local_addr()?;
        Ok(Self {
            inner: endpoint,
            local_addr,
        })
    }

    /// Build a client endpoint bound to an ephemeral loopback port,
    /// trusting the supplied root certificates.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if the UDP socket fails to bind, or if the
    /// rustls verifier cannot be built (wrapped as
    /// [`io::ErrorKind::Other`]).
    pub fn client_on_loopback(roots: Arc<RootCertStore>) -> io::Result<Self> {
        let client_cfg =
            quinn::ClientConfig::with_root_certificates(roots).map_err(io::Error::other)?;
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
        let mut endpoint = quinn::Endpoint::client(addr)?;
        endpoint.set_default_client_config(client_cfg);
        let local_addr = endpoint.local_addr()?;
        Ok(Self {
            inner: endpoint,
            local_addr,
        })
    }

    /// The socket address the endpoint is bound to.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Access the underlying [`quinn::Endpoint`] for advanced use.
    #[must_use]
    pub const fn inner(&self) -> &quinn::Endpoint {
        &self.inner
    }
}

/// Drive one unreliable-datagram roundtrip through real quinn endpoints.
///
/// The client connects to the server, sends `dg.data` as a QUIC datagram,
/// and the returned [`QuicDatagram`] carries the bytes the server actually
/// received plus the original `connection_id`.
///
/// # Errors
///
/// Any quinn-level failure — UDP I/O, TLS handshake, datagram send or
/// receive — is surfaced as a [`QuicError`] variant. An empty `dg.data` is
/// rejected up front with [`QuicError::EmptyPayload`].
pub async fn roundtrip_datagram(
    server: &QuicEndpoint,
    client: &QuicEndpoint,
    dg: QuicDatagram,
) -> Result<QuicDatagram, QuicError> {
    if dg.data.is_empty() {
        return Err(QuicError::EmptyPayload);
    }
    let server_ep = server.inner.clone();
    let server_task = tokio::spawn(async move {
        let incoming = server_ep.accept().await.ok_or(QuicError::NoIncoming)?;
        let conn = incoming.await?;
        let bytes = conn.read_datagram().await?;
        Ok::<bytes::Bytes, QuicError>(bytes)
    });

    let conn = client
        .inner
        .connect(server.local_addr, "127.0.0.1")?
        .await?;
    conn.send_datagram(bytes::Bytes::from(dg.data.clone()))?;

    let received = server_task
        .await
        .map_err(|e| QuicError::Internal(e.to_string()))??;
    conn.close(0u32.into(), b"ok");

    Ok(QuicDatagram {
        connection_id: dg.connection_id,
        data: received.to_vec(),
    })
}

/// Drive one unidirectional-stream roundtrip through real quinn endpoints.
///
/// The client opens a uni stream, writes `s.data`, and finishes the stream;
/// the server accepts the uni stream and reads it to EOF. The returned
/// [`QuicStream`] preserves `stream_id` and `fin` from the input and
/// carries the bytes the server actually received.
///
/// # Errors
///
/// Any quinn-level failure (UDP I/O, TLS handshake, stream I/O) is
/// surfaced as a [`QuicError`] variant. An empty `s.data` is rejected up
/// front with [`QuicError::EmptyStreamData`].
pub async fn roundtrip_stream(
    server: &QuicEndpoint,
    client: &QuicEndpoint,
    s: QuicStream,
) -> Result<QuicStream, QuicError> {
    if s.data.is_empty() {
        return Err(QuicError::EmptyStreamData);
    }
    let server_ep = server.inner.clone();
    let server_task = tokio::spawn(async move {
        let incoming = server_ep.accept().await.ok_or(QuicError::NoIncoming)?;
        let conn = incoming.await?;
        let mut recv = conn.accept_uni().await?;
        // 16 MiB read cap — tests send a handful of bytes; larger payloads
        // will be routed differently in Pillar 3b.
        let data = recv.read_to_end(16 * 1024 * 1024).await?;
        Ok::<Vec<u8>, QuicError>(data)
    });

    let conn = client
        .inner
        .connect(server.local_addr, "127.0.0.1")?
        .await?;
    let mut send = conn.open_uni().await?;
    send.write_all(&s.data).await?;
    send.finish()
        .map_err(|e| QuicError::Internal(e.to_string()))?;
    drop(send);

    let received = server_task
        .await
        .map_err(|e| QuicError::Internal(e.to_string()))??;
    conn.close(0u32.into(), b"ok");

    Ok(QuicStream {
        stream_id: s.stream_id,
        data: received,
        fin: s.fin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datagram_forward_ok() {
        let dg = QuicDatagram {
            connection_id: 42,
            data: b"hello".to_vec(),
        };
        let fwd = forward_datagram(&dg).unwrap();
        assert_eq!(fwd.connection_id, 42);
        assert_eq!(fwd.data, b"hello");
    }

    #[test]
    fn datagram_empty_rejected() {
        let dg = QuicDatagram {
            connection_id: 1,
            data: vec![],
        };
        assert!(forward_datagram(&dg).is_err());
    }

    #[test]
    fn stream_forward_ok() {
        let stream = QuicStream {
            stream_id: 1,
            data: b"payload".to_vec(),
            fin: true,
        };
        let fwd = forward_stream(&stream).unwrap();
        assert_eq!(fwd.stream_id, 1);
        assert!(fwd.fin);
    }

    #[test]
    fn stream_empty_rejected() {
        let stream = QuicStream {
            stream_id: 0,
            data: vec![],
            fin: false,
        };
        assert!(forward_stream(&stream).is_err());
    }
}
