//! INC-2V Task F — R13 burst: >=50 iterations of the full WS-over-H2
//! (RFC 8441) upgrade + relay + clean close cycle, real wire, asserting no
//! file-descriptor / connection leak across iterations.
//!
//! Each iteration: TLS+ALPN-h2 connect, extended CONNECT (200), a Text +
//! Binary round-trip through the gateway to a real tungstenite echo backend,
//! then a clean client-initiated Close. Between the start and end we sample
//! the process open-fd count (/proc/self/fd) and assert it did not grow
//! monotonically with iteration count (a per-iteration leak of even one fd
//! would accumulate to >=50).

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
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

fn open_fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .map(|d| d.count())
        .unwrap_or(0)
}

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let g = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    (
        vec![CertificateDer::from(g.cert.der().to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(g.signing_key.serialize_der())),
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

async fn spawn_gateway(backend: SocketAddr) -> (SocketAddr, CertificateDer<'static>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend]).unwrap());
    let h2 = Arc::new(
        H2Proxy::new(pool, picker as _, None, HttpTimeouts::default(), true)
            .with_websocket(Arc::new(WsProxy::new(WsConfig {
                idle_timeout: Duration::from_secs(30),
                max_message_size: 1024 * 1024,
                enabled: true,
                ..WsConfig::default()
            })))
            // CF-S27-2: WS-over-H2 is opt-in; this R13 burst proof needs it on.
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

async fn one_cycle(gw: SocketAddr, ta: CertificateDer<'static>) {
    let sock = TcpStream::connect(gw).await.unwrap();
    let connector = TlsConnector::from(build_client_cfg(ta));
    let sn = ServerName::try_from(SAN_HOST).unwrap();
    let tls = connector.connect(sn, sock).await.unwrap();
    let (h2c, conn) = h2::client::handshake(tls).await.unwrap();
    let conn_task = tokio::spawn(async move {
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
    let resp = tokio::time::timeout(STEP_TIMEOUT, resp_fut)
        .await
        .expect("resp timeout")
        .expect("resp err");
    assert_eq!(resp.status(), http::StatusCode::OK);
    let adapter = H2StreamAdapter {
        send,
        recv: resp.into_body(),
        leftover: Bytes::new(),
    };
    let mut ws: WebSocketStream<H2StreamAdapter> = WebSocketStream::from_raw_socket(
        adapter,
        Role::Client,
        Some(WsConfig::default().tungstenite_config()),
    )
    .await;

    ws.send(Message::Text("ping".into())).await.unwrap();
    let echo = tokio::time::timeout(STEP_TIMEOUT, ws.next())
        .await
        .expect("text echo timeout")
        .expect("stream ended")
        .expect("text echo err");
    assert!(matches!(echo, Message::Text(ref s) if s.as_str() == "ping"));

    let payload: Vec<u8> = (0..1024).map(|i| (i & 0xff) as u8).collect();
    ws.send(Message::Binary(payload.clone().into()))
        .await
        .unwrap();
    let echo = tokio::time::timeout(STEP_TIMEOUT, ws.next())
        .await
        .expect("bin echo timeout")
        .expect("stream ended")
        .expect("bin echo err");
    assert!(matches!(echo, Message::Binary(ref b) if *b == payload));

    // Clean client-initiated close; drain to terminal item.
    ws.close(None).await.unwrap();
    let _ = tokio::time::timeout(STEP_TIMEOUT, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => continue,
                Some(Err(_)) => break,
            }
        }
    })
    .await;
    drop(ws);
    conn_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_h2_upgrade_relay_close_burst_no_leak() {
    const ITERS: usize = 60; // >= 50 per the R13 bar
    let backend = spawn_echo_backend().await;
    let (gw, ta) = spawn_gateway(backend).await;

    // Warm up a few cycles so steady-state caches/allocations settle, then
    // record the baseline fd count.
    for _ in 0..5 {
        one_cycle(gw, ta.clone()).await;
    }
    // Give async teardown a beat to reclaim sockets.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let fd_baseline = open_fd_count();

    for i in 0..ITERS {
        one_cycle(gw, ta.clone()).await;
        if i % 15 == 0 {
            eprintln!("burst iter {i}: open fds = {}", open_fd_count());
        }
    }
    // Let teardown settle.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let fd_after = open_fd_count();
    eprintln!("R13 burst: {ITERS} cycles, fd baseline={fd_baseline} after={fd_after}");

    // A per-iteration fd leak of even 1 would accumulate to >= ITERS. Allow
    // generous slack (32) for steady-state pool sockets / runtime churn.
    assert!(
        fd_after <= fd_baseline + 32,
        "R13 fd LEAK: open fds grew from {fd_baseline} to {fd_after} across {ITERS} \
         upgrade/relay/close cycles (>{}=slack) — a per-cycle socket/fd leak",
        fd_baseline + 32
    );
}
