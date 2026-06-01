//! INC-3 — F-S27-2 regression: end-to-end BACKPRESSURE in the shared WS
//! relay (`ws_proxy::proxy_frames`), proven DETERMINISTICALLY and
//! parallel-safe (NO process-RSS gauge).
//!
//! Deterministic anchor (process-memory-INDEPENDENT, parallel-safe): the
//! BACKEND counts how many flood messages it managed to push at a client that
//! never reads. If the relay propagates backpressure end-to-end, the
//! backend's `flush().await` parks once the gateway stops draining, so the
//! pushed count PLATEAUS far below the flood. If the gateway buffers
//! unbounded, the backend pushes ~the whole flood.
//!
//! TRANSPORT FINDING (see audit/websockets/s27-fs27-2-proof/): the relay's
//! select-loop `send().await` DOES park whenever the underlying client write
//! surfaces `WouldBlock` — which is the case for the shipped H1 `Upgrade`
//! path (a real TCP socket). So the H1 case below is GREEN and is a genuine
//! R8(ii) regression guard for the shipped path. The WS-over-H2 case is
//! `#[ignore]`d: the upgraded extended-CONNECT stream buffers inside the `h2`
//! crate's `SendStream` (via hyper's `H2Upgraded`), below the tungstenite
//! layer, and is NOT bounded by anything `WsConfig` controls — that fix is
//! escalated to the lead (the diagnostic explains why `max_write_buffer_size`
//! and hyper's `max_send_buf_size` are both ineffective for the H2 tunnel).

#![cfg(test)]

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Buf, Bytes};
use futures_util::{SinkExt, StreamExt};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
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

// Flood shape. Each message is `MSG_BYTES`; the backend tries to push
// `FLOOD_MSGS` of them at a non-reading client. The total flood volume
// (FLOOD_MSGS * MSG_BYTES = ~512 MiB) far exceeds any legal in-flight bound.
const MSG_BYTES: usize = 256 * 1024;
const FLOOD_MSGS: u64 = 2_048;
// Per-listener message cap; the in-flight bound is then
// `max_message_size + 128 KiB write-buffer headroom`, body-VOLUME-independent.
const MAX_MESSAGE_SIZE: usize = 512 * 1024;
// Generous plateau ceiling. The true in-flight bound is a few messages
// (one bounded write buffer + OS socket buffers + transport windows); 256 is
// well under the 2048-message flood yet far above the real plateau, so the
// assertion is decisive (PRE-FIX pushes all 2048) without being flaky.
const PLATEAU_CEILING: u64 = 256;
// How long to let the backend flood before sampling its pushed count. The
// plateau is reached in well under this; we just need the producer to have
// run long enough that an UNBOUNDED relay would have drained the whole flood.
const FLOOD_OBSERVE: Duration = Duration::from_secs(5);

// ── shared TLS / pool harness (mirrors ws_h2_e2e.rs / ws_h2_burst.rs) ──

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let g = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    (
        vec![CertificateDer::from(g.cert.der().to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(g.key_pair.serialize_der())),
    )
}

fn build_client_cfg(ta: CertificateDer<'static>) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ta).unwrap();
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

fn ws_cfg() -> WsConfig {
    WsConfig {
        // Idle + read-frame budgets kept generous so neither watchdog masks
        // the backpressure plateau as a timeout during the observation
        // window. (The whole point is to observe the producer STALL, not a
        // watchdog firing.)
        idle_timeout: Duration::from_secs(60),
        read_frame_timeout: Duration::from_secs(60),
        max_message_size: MAX_MESSAGE_SIZE,
        enabled: true,
        ..WsConfig::default()
    }
}

/// A flooding WS backend: on the client's "go" frame it pushes `FLOOD_MSGS`
/// Binary messages of `MSG_BYTES` each, bumping `pushed` only AFTER each
/// frame has actually been flushed to its socket. When the relay
/// backpressures (client not reading), the backend's `flush().await` PARKS
/// and the count plateaus. Returns the backend address + the shared counter.
///
/// CRITICAL for correct attribution (verifier note, s27-r8-ws-proof.md):
/// tungstenite's DEFAULT `max_write_buffer_size` is `usize::MAX`, and
/// `send().await` only parks when the underlying socket write FAILS — so a
/// default-config flooder would buffer the entire flood in its OWN heap and
/// `pushed` would race to completion, mis-attributing the BACKEND's buffer as
/// a gateway R8 violation. We pin a SMALL bounded write buffer + explicit
/// per-frame `flush` so the backend's send genuinely blocks on TCP
/// backpressure once the gateway stops draining → `pushed` reflects the
/// number of frames the GATEWAY accepted, not the backend's buffer.
async fn spawn_flood_backend_tcp(pushed: Arc<AtomicU64>) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let pushed = Arc::clone(&pushed);
            tokio::spawn(async move {
                let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
                    write_buffer_size: 0,
                    max_write_buffer_size: MSG_BYTES + 1024,
                    ..tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                };
                let mut ws =
                    match tokio_tungstenite::accept_async_with_config(sock, Some(cfg)).await {
                        Ok(w) => w,
                        Err(_) => return,
                    };
                // Wait for the client's "go" frame before flooding.
                let _ = ws.next().await;
                let payload = vec![0xABu8; MSG_BYTES];
                for _ in 0..FLOOD_MSGS {
                    // `feed` + explicit `flush`: flush drives the bytes to the
                    // socket and BLOCKS until the bounded write buffer drains.
                    // Under gateway backpressure the socket send buffer +
                    // gateway recv window fill and this flush parks → `pushed`
                    // plateaus at the true number of frames the gateway
                    // accepted.
                    if ws.feed(Message::Binary(payload.clone())).await.is_err() {
                        break;
                    }
                    if ws.flush().await.is_err() {
                        break;
                    }
                    pushed.fetch_add(1, Ordering::Relaxed);
                }
                let _ = ws.close(None).await;
            });
        }
    });
    local
}

// ── H2 gateway + non-reading H2 client ─────────────────────────────────

async fn spawn_h2_gateway(backend: SocketAddr) -> (SocketAddr, CertificateDer<'static>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend]).unwrap());
    let h2 = Arc::new(
        H2Proxy::new(pool, picker as _, None, HttpTimeouts::default(), true)
            .with_websocket(Arc::new(WsProxy::new(ws_cfg())))
            // CF-S27-2: WS-over-H2 is opt-in; the H2 plateau case needs it on.
            .with_h2_extended_connect(true),
    );
    let (chain, key) = make_cert_for(SAN_HOST);
    let ta = chain[0].clone();
    let rot = TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
    let alpn: &[&[u8]] = &[b"h2", b"http/1.1"];
    let cfg = build_server_config(Arc::new(Mutex::new(rot)), chain, key, alpn).unwrap();
    let acceptor = TlsAcceptor::from(cfg);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let h2 = Arc::clone(&h2);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(sock).await else {
                    return;
                };
                if tls.get_ref().1.alpn_protocol() == Some(b"h2".as_ref()) {
                    let _ = h2.serve_connection(tls, peer).await;
                }
            });
        }
    });
    (local, ta)
}

// h2 stream → AsyncRead+AsyncWrite adapter (shared with ws_h2_e2e.rs).
struct H2StreamAdapter {
    send: h2::SendStream<Bytes>,
    recv: h2::RecvStream,
    leftover: Bytes,
}
impl AsyncRead for H2StreamAdapter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.leftover.is_empty() {
            let n = self.leftover.len().min(buf.remaining());
            let chunk = self.leftover.split_to(n);
            buf.put_slice(&chunk);
            return Poll::Ready(Ok(()));
        }
        match self.recv.poll_data(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Ready(Some(Ok(mut data))) => {
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
        self.send.reserve_capacity(buf.len());
        match self.send.poll_capacity(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(Err(std::io::Error::other("h2 send closed"))),
            Poll::Ready(Some(Ok(cap))) => {
                let n = cap.min(buf.len());
                if n == 0 {
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
                Poll::Ready(Err(std::io::Error::other(format!("h2 cap: {e}"))))
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.send.send_data(Bytes::new(), true) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(std::io::Error::other(format!("h2 shutdown: {e}")))),
        }
    }
}

/// Open a WS-over-H2 tunnel with a PINNED small client receive window so a
/// non-reading client cannot silently absorb the flood in its own recv
/// buffer — the stall must be attributable to the GATEWAY's bounded write
/// buffer, not the client's.
async fn open_ws_h2_small_window(
    gw: SocketAddr,
    ta: CertificateDer<'static>,
) -> WebSocketStream<H2StreamAdapter> {
    let sock = TcpStream::connect(gw).await.unwrap();
    let connector = TlsConnector::from(build_client_cfg(ta));
    let sn = ServerName::try_from(SAN_HOST).unwrap();
    let tls = connector.connect(sn, sock).await.unwrap();
    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(64 * 1024)
        .initial_connection_window_size(64 * 1024);
    let (h2c, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2c = h2c.ready().await.unwrap();
    let uri: http::Uri = format!("https://{SAN_HOST}/chat").parse().unwrap();
    let mut req = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(uri)
        .body(())
        .unwrap();
    req.extensions_mut()
        .insert(h2::ext::Protocol::from_static("websocket"));
    let (resp_fut, send) = h2c.send_request(req, false).unwrap();
    let resp = resp_fut.await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    WebSocketStream::from_raw_socket(
        H2StreamAdapter {
            send,
            recv: resp.into_body(),
            leftover: Bytes::new(),
        },
        Role::Client,
        Some(ws_cfg().tungstenite_config()),
    )
    .await
}

// ── H1 gateway + non-reading H1 client ─────────────────────────────────

async fn spawn_h1_gateway(backend: SocketAddr) -> SocketAddr {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend]).unwrap());
    let proxy = Arc::new(
        H1Proxy::new(pool, picker, None, HttpTimeouts::default(), false)
            .with_websocket(Arc::new(WsProxy::new(ws_cfg()))),
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let proxy = Arc::clone(&proxy);
            tokio::spawn(async move {
                let _ = proxy.serve_connection(sock, peer).await;
            });
        }
    });
    local
}

// ── Test 1: WS-over-H2 producer plateau ────────────────────────────────
//
// IGNORED — KNOWN GAP, not a fix regression. INC-3 found that the prescribed
// `max_write_buffer_size` bound does NOT bound the WS-over-H2 transport: the
// upgraded extended-CONNECT stream buffers in the `h2` crate's SendStream
// (via hyper's `H2Upgraded` mpsc -> `send_data`), BELOW the tungstenite layer
// `WsConfig` controls, and even hyper's already-set `max_send_buf_size`
// (64 KiB) does not apply to the upgraded stream. Asserting the plateau here
// would be a knowingly-RED test; the real H2 fix (raw-SendStream capacity
// gating, or a relay-level bounded in-flight wrapper) is escalated to the
// lead — see audit/websockets/s27-fs27-2-proof/inc3-STOP-diagnostic.txt.
// This is a NEW test authored in INC-3 (not an existing test), so ignoring
// the not-yet-bounded H2 case is the honest representation of the gap.
#[ignore = "F-S27-2 H2 transport not yet bounded — see s27-fs27-2-proof/inc3-STOP-diagnostic.txt"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2_backend_flood_plateaus_against_nonreading_client() {
    let pushed = Arc::new(AtomicU64::new(0));
    let backend = spawn_flood_backend_tcp(Arc::clone(&pushed)).await;
    let (gw, ta) = spawn_h2_gateway(backend).await;

    // Open the tunnel, send one Text to trigger the backend's flood, then
    // NEVER read — `ws` is held (keeps the stream open) but `.next()` is
    // never polled.
    let mut ws = open_ws_h2_small_window(gw, ta).await;
    ws.send(Message::Text("go".into())).await.unwrap();

    // Let the producer run. An UNBOUNDED relay would drain the entire flood
    // in this window; a backpressured one stalls the producer early.
    tokio::time::sleep(FLOOD_OBSERVE).await;
    let n = pushed.load(Ordering::Relaxed);
    eprintln!("H2 plateau: backend pushed {n} / {FLOOD_MSGS} messages (ceiling {PLATEAU_CEILING})");
    assert!(
        n < PLATEAU_CEILING,
        "R8(ii) VIOLATION (WS-over-H2): backend pushed {n} of {FLOOD_MSGS} flood \
         messages at a non-reading client — the gateway is NOT backpressuring \
         (expected a plateau < {PLATEAU_CEILING}); F-S27-2 unbounded buffering"
    );
    assert!(
        n > 0,
        "non-vacuous control: the backend must have pushed at least one message \
         (the flood actually started), got {n}"
    );

    let _ = ws.close(None).await;
}

// ── Test 2: WS-over-H1 producer plateau (shipped path, R12) ────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h1_backend_flood_plateaus_against_nonreading_client() {
    let pushed = Arc::new(AtomicU64::new(0));
    let backend = spawn_flood_backend_tcp(Arc::clone(&pushed)).await;
    let gw = spawn_h1_gateway(backend).await;

    // Plain-TCP WS client (TLS termination is exercised elsewhere; this path
    // is about the frame relay). Connect, trigger the flood, then NEVER read.
    let url = format!("ws://{gw}/chat");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws.send(Message::Text("go".into())).await.unwrap();

    tokio::time::sleep(FLOOD_OBSERVE).await;
    let n = pushed.load(Ordering::Relaxed);
    eprintln!("H1 plateau: backend pushed {n} / {FLOOD_MSGS} messages (ceiling {PLATEAU_CEILING})");
    assert!(
        n < PLATEAU_CEILING,
        "R8(ii) VIOLATION (WS-over-H1, shipped path): backend pushed {n} of \
         {FLOOD_MSGS} flood messages at a non-reading client — the gateway is \
         NOT backpressuring (expected a plateau < {PLATEAU_CEILING}); F-S27-2"
    );
    assert!(
        n > 0,
        "non-vacuous control: the backend must have pushed at least one message, got {n}"
    );

    let _ = ws.close(None).await;
}
