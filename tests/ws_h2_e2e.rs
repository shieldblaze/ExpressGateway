//! Real-wire end-to-end test for WebSocket-over-HTTP/2 (RFC 8441 extended
//! CONNECT), INC-1 of SESSION 27.
//!
//! The codebase already implements the WS-over-H2 path:
//! `H2Proxy::serve_connection` advertises `SETTINGS_ENABLE_CONNECT_PROTOCOL`
//! (`builder.enable_connect_protocol()`), detects an RFC 8441 extended
//! CONNECT via `crate::ws_proxy::is_h2_extended_connect`, and
//! `handle_ws_extended_connect` returns `200 OK`, then dials an H1
//! WebSocket backend and runs the shared relay `WsProxy::proxy_frames`.
//!
//! Until now the only "test" of that path was a source-grep code-presence
//! check (`tests/h2_connect_protocol_settings.rs`). This file is the genuine
//! one: nothing about the gateway is mocked.
//!
//! Wire shape exercised here:
//!
//!   raw `h2` client ──TLS(ALPN h2)──▶ H2Proxy ──TCP──▶ tungstenite echo backend
//!        │                              │                       │
//!        │  extended CONNECT (:protocol=websocket)              │
//!        │ ───────────────────────────▶│                       │
//!        │  200 OK (stream stays open as the tunnel)            │
//!        │ ◀───────────────────────────│                       │
//!        │  RFC 6455 frames inside H2 DATA frames ◀────────────▶│
//!
//! On the client side we wrap the open H2 stream's `SendStream<Bytes>` +
//! `RecvStream` as an `AsyncRead + AsyncWrite` adapter and run a real
//! `tokio_tungstenite` `Role::Client` over it — i.e. the realistic
//! WS-over-H2 client (relay approach (a) in the INC-1 brief). The bytes the
//! adapter writes are genuine masked RFC 6455 client frames; the gateway
//! wrapped its end as `Role::Server`, so the framing must be valid on the
//! wire for the round-trip to succeed.

#![cfg(test)]

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Buf, Bytes};
use futures_util::{SinkExt, StreamExt};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use lb_l7::ws_proxy::{WsConfig, WsProxy};
use lb_security::{TicketRotator, build_server_config};
use parking_lot::Mutex;
use rustls::ClientConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

const SAN_HOST: &str = "expressgateway.test";

// Generous-but-bounded ceiling on any single await. WS-over-H2 happy-path
// round-trips complete in milliseconds on loopback; this is purely a
// deadlock tripwire so a regression lands in red instead of hanging the
// suite.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

// ── TLS / pool harness (mirrors tests/h2_security_live.rs) ─────────────

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    let cert_der = generated.cert.der().to_vec();
    let key_der = generated.signing_key.serialize_der();
    (
        vec![CertificateDer::from(cert_der)],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
    )
}

fn build_client_cfg(trust_anchor: CertificateDer<'static>) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(trust_anchor).unwrap();
    let mut cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(Arc::new(roots))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(cfg)
}

fn build_pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts {
            nodelay: true,
            keepalive: false,
            rcvbuf: None,
            sndbuf: None,
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    )
}

/// Spawn a real H1 WebSocket echo backend (the `spawn_echo_backend` shape
/// from `tests/ws_proxy_e2e.rs`). Text/Binary are echoed verbatim; Close is
/// forwarded (we break and let tungstenite's state machine reply).
async fn spawn_echo_backend() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let ws = match tokio_tungstenite::accept_async(sock).await {
                    Ok(w) => w,
                    Err(_) => return,
                };
                let (mut tx, mut rx) = ws.split();
                while let Some(Ok(msg)) = rx.next().await {
                    // See ws_proxy_e2e.rs for why the echo lives inside the
                    // match arm rather than a guard (rust-1.95.0 clippy).
                    #[allow(clippy::collapsible_match)]
                    match msg {
                        Message::Text(_) | Message::Binary(_) => {
                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
                let _ = tx.close().await;
            });
        }
    });
    local
}

/// Spawn the gateway: an `H2Proxy` with WebSocket enabled, pointed at
/// `backend_addr`, served over TLS with ALPN `h2`. Returns the bound
/// gateway address + the trust anchor for the client.
async fn spawn_h2_ws_gateway(backend_addr: SocketAddr) -> (SocketAddr, CertificateDer<'static>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let h2_proxy = Arc::new(
        H2Proxy::new(pool, picker as _, None, HttpTimeouts::default(), true)
            .with_websocket(Arc::new(WsProxy::new(WsConfig {
                idle_timeout: Duration::from_secs(30),
                max_message_size: 1024 * 1024,
                enabled: true,
                ..WsConfig::default()
            })))
            // CF-S27-2: WS-over-H2 is opt-in; this test proves it works WHEN
            // enabled, so flip the gate ON.
            .with_h2_extended_connect(true),
    );

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let rot = TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
    let rot_arc = Arc::new(Mutex::new(rot));
    let alpn: &[&[u8]] = &[b"h2", b"http/1.1"];
    let server_cfg = build_server_config(rot_arc, cert_chain, key, alpn).unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let h2 = Arc::clone(&h2_proxy);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(sock).await else {
                    return;
                };
                let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                if alpn.as_deref() == Some(b"h2".as_ref()) {
                    let _ = h2.serve_connection(tls, peer).await;
                }
            });
        }
    });

    (local, trust_anchor)
}

// ── h2 stream → AsyncRead + AsyncWrite adapter (relay approach (a)) ─────

/// Bridges an open H2 stream (`SendStream<Bytes>` for the request body
/// direction, `RecvStream` for the response body direction) into the
/// `AsyncRead + AsyncWrite` surface `tokio_tungstenite` requires.
///
/// * `AsyncRead` drains inbound H2 DATA frames, copying bytes out of the
///   `RecvStream` and *releasing flow-control capacity* as it consumes them
///   (`flow_control().release_capacity(n)`) so the server is never starved
///   of a window to push the echo back.
/// * `AsyncWrite` reserves capacity then ships the buffer as an H2 DATA
///   frame via `send_data(buf, /*end_of_stream=*/ false)`; `poll_shutdown`
///   sends the empty end-of-stream DATA frame (half-close).
struct H2StreamAdapter {
    send: h2::SendStream<Bytes>,
    recv: h2::RecvStream,
    /// Leftover bytes from a DATA frame larger than the caller's read buf.
    leftover: Bytes,
}

impl H2StreamAdapter {
    fn new(send: h2::SendStream<Bytes>, recv: h2::RecvStream) -> Self {
        Self {
            send,
            recv,
            leftover: Bytes::new(),
        }
    }
}

impl AsyncRead for H2StreamAdapter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // Drain any leftover from a prior oversized DATA frame first.
        if !self.leftover.is_empty() {
            let n = self.leftover.len().min(buf.remaining());
            let chunk = self.leftover.split_to(n);
            buf.put_slice(&chunk);
            return Poll::Ready(Ok(()));
        }
        match self.recv.poll_data(cx) {
            Poll::Pending => Poll::Pending,
            // End of the response stream → clean EOF for tungstenite.
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Ready(Some(Ok(mut data))) => {
                // CRITICAL: release the consumed bytes back to the H2 flow-
                // control window or the peer stalls once the initial window
                // is spent. We release the *whole* frame even when the
                // caller's buffer is smaller, because we have taken
                // ownership of all of it (the tail lives in `leftover`).
                let len = data.len();
                let _ = self.recv.flow_control().release_capacity(len);
                let n = len.min(buf.remaining());
                buf.put_slice(&data[..n]);
                data.advance(n);
                self.leftover = data;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Err(std::io::Error::other(format!("h2 recv: {e}"))))
            }
        }
    }
}

impl AsyncWrite for H2StreamAdapter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        // Reserve send-window capacity for the whole buffer, then wait for
        // it to be granted. `reserve_capacity` is idempotent-ish: it sets
        // the desired reservation; `poll_capacity` resolves once some is
        // available.
        self.send.reserve_capacity(buf.len());
        match self.send.poll_capacity(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(Err(std::io::Error::other(
                "h2 send stream closed before capacity",
            ))),
            Poll::Ready(Some(Ok(cap))) => {
                let n = cap.min(buf.len());
                if n == 0 {
                    // No capacity granted this turn; ask again.
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                let chunk = Bytes::copy_from_slice(&buf[..n]);
                match self.send.send_data(chunk, false) {
                    Ok(()) => Poll::Ready(Ok(n)),
                    Err(e) => Poll::Ready(Err(std::io::Error::other(format!("h2 send: {e}")))),
                }
            }
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Err(std::io::Error::other(format!("h2 capacity: {e}"))))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // H2 DATA frames are flushed by the connection task as soon as
        // `send_data` enqueues them; there is no user-visible buffer here.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // Half-close: an empty end-of-stream DATA frame.
        match self.send.send_data(Bytes::new(), true) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(std::io::Error::other(format!("h2 shutdown: {e}")))),
        }
    }
}

// ── Driving the raw h2 client + extended CONNECT ───────────────────────

/// Open the gateway TLS+H2 connection, send an RFC 8441 extended CONNECT
/// for `path`, assert 200, and return a post-handshake tungstenite
/// `Role::Client` riding the now-open H2 stream.
///
/// `conn_task` (the h2 connection driver) is detached; the returned
/// `WebSocketStream` keeps the connection alive for the duration of the
/// test.
async fn open_ws_over_h2(
    gateway: SocketAddr,
    trust_anchor: CertificateDer<'static>,
    path: &str,
) -> WebSocketStream<H2StreamAdapter> {
    // TLS connect with ALPN h2.
    let sock = TcpStream::connect(gateway).await.unwrap();
    let connector = TlsConnector::from(build_client_cfg(trust_anchor));
    let sn = ServerName::try_from(SAN_HOST).unwrap();
    let tls = connector.connect(sn, sock).await.unwrap();
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(b"h2".as_ref()),
        "client must negotiate ALPN h2"
    );

    // Raw h2 handshake.
    let (h2, conn) = tokio::time::timeout(STEP_TIMEOUT, h2::client::handshake(tls))
        .await
        .expect("h2 handshake timed out")
        .expect("h2 handshake failed");
    tokio::spawn(async move {
        // Drive the connection to completion; ignore the terminal result
        // (clean GOAWAY at end of test is expected).
        let _ = conn.await;
    });
    let mut h2 = tokio::time::timeout(STEP_TIMEOUT, h2.ready())
        .await
        .expect("h2 ready timed out")
        .expect("h2 connection not ready");

    // Build the extended CONNECT: CONNECT + authority + path + the
    // `:protocol = websocket` pseudo-header (carried as the typed request
    // extension `h2::ext::Protocol`).
    let uri: http::Uri = format!("https://{SAN_HOST}{path}").parse().unwrap();
    let mut req = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(uri)
        .body(())
        .unwrap();
    req.extensions_mut()
        .insert(h2::ext::Protocol::from_static("websocket"));

    // end_of_stream = false → the stream stays open as the tunnel.
    let (resp_fut, send_stream) = h2
        .send_request(req, false)
        .expect("send extended CONNECT failed");

    let resp = tokio::time::timeout(STEP_TIMEOUT, resp_fut)
        .await
        .expect("extended CONNECT response timed out")
        .expect("extended CONNECT response errored");
    assert_eq!(
        resp.status(),
        http::StatusCode::OK,
        "gateway must answer extended CONNECT with 200"
    );

    let recv_stream = resp.into_body();
    let adapter = H2StreamAdapter::new(send_stream, recv_stream);
    // Role::Client: the client side masks its frames per RFC 6455; the
    // gateway wrapped its end as Role::Server.
    WebSocketStream::from_raw_socket(
        adapter,
        Role::Client,
        Some(WsConfig::default().tungstenite_config()),
    )
    .await
}

// ── Test: full happy-path round-trip over WS-over-H2 ───────────────────

/// Text echo + Binary echo + clean client-initiated Close, all over a real
/// RFC 8441 WS-over-H2 tunnel terminated by the gateway and relayed to a
/// real H1 tungstenite echo backend.
#[tokio::test]
async fn ws_over_h2_text_binary_roundtrip_then_close() {
    let backend = spawn_echo_backend().await;
    let (gw, anchor) = spawn_h2_ws_gateway(backend).await;

    let mut client = open_ws_over_h2(gw, anchor, "/chat").await;

    // 1) Text round-trip.
    client
        .send(Message::Text("hello-h2-ws".into()))
        .await
        .expect("send Text over WS-over-H2 failed");
    let msg = tokio::time::timeout(STEP_TIMEOUT, client.next())
        .await
        .expect("timed out waiting for Text echo")
        .expect("client stream ended before Text echo")
        .expect("Text echo was an error");
    match msg {
        Message::Text(s) => assert_eq!(s, "hello-h2-ws"),
        other => panic!("expected Text echo, got {other:?}"),
    }

    // 2) Binary round-trip — 4 KiB of non-repeating bytes so a short-circuit
    //    echo cannot pass by accident.
    let payload: Vec<u8> = (0..4096).map(|i| (i & 0xff) as u8).collect();
    client
        .send(Message::Binary(payload.clone().into()))
        .await
        .expect("send Binary over WS-over-H2 failed");
    let msg = tokio::time::timeout(STEP_TIMEOUT, client.next())
        .await
        .expect("timed out waiting for Binary echo")
        .expect("client stream ended before Binary echo")
        .expect("Binary echo was an error");
    match msg {
        Message::Binary(b) => assert_eq!(b, payload),
        other => panic!("expected Binary echo, got {other:?}"),
    }

    // 3) Client-initiated Close → the stream must end cleanly. Sending the
    //    Close drives tungstenite's closing handshake; draining `next()` to
    //    None proves a clean end (no error).
    client
        .close(None)
        .await
        .expect("client close handshake failed");
    let tail = tokio::time::timeout(STEP_TIMEOUT, async {
        loop {
            match client.next().await {
                // tungstenite surfaces the peer's Close echo (or None) as
                // the terminal item; both are clean ends.
                Some(Ok(Message::Close(_))) | None => break Ok(()),
                Some(Ok(_)) => continue, // drain any late echo
                Some(Err(e)) => break Err(e),
            }
        }
    })
    .await
    .expect("timed out draining the closing handshake");
    tail.expect("closing handshake ended with a tungstenite error");
}
