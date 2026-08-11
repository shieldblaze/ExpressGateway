//! Sustained load drivers, one per datapath. Each `run_*_load` spawns `concurrency` workers
//! that loop a unit of work until the [`CancellationToken`] fires, tallying ok/err. The goal is
//! sustained churning concurrency — NOT throughput — so workers favour connection turnover.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::{TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;

const MAX_UDP: usize = 65_535;

/// Shared success/error tally for a load driver (liveness + sanity, not a throughput SLO).
#[derive(Debug, Default)]
pub struct LoadStats {
    ok: AtomicU64,
    err: AtomicU64,
}

impl LoadStats {
    /// A fresh tally.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Record a successful unit of work.
    pub fn ok(&self) {
        self.ok.fetch_add(1, Ordering::Relaxed);
    }
    /// Record a failed unit of work.
    pub fn err(&self) {
        self.err.fetch_add(1, Ordering::Relaxed);
    }
    /// Successful units so far.
    #[must_use]
    pub fn ok_count(&self) -> u64 {
        self.ok.load(Ordering::Relaxed)
    }
    /// Failed units so far.
    #[must_use]
    pub fn err_count(&self) -> u64 {
        self.err.load(Ordering::Relaxed)
    }
}

/// Body sizes cycled through to vary upload/relay pressure.
const BODY_SIZES: [usize; 4] = [0, 256, 4096, 65_536];

fn body_for(i: u64) -> Full<Bytes> {
    let len = BODY_SIZES[(i as usize) % BODY_SIZES.len()];
    if len == 0 {
        Full::new(Bytes::new())
    } else {
        Full::new(Bytes::from(vec![b'x'; len]))
    }
}

/// H1 keep-alive + churn load.
pub async fn run_h1_load(
    target: SocketAddr,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        workers.push(tokio::spawn(async move {
            let mut iter = w as u64;
            while !cancel.is_cancelled() {
                iter += 1;
                match h1_keepalive_burst(target, iter, 5).await {
                    Ok(n) => {
                        for _ in 0..n {
                            stats.ok();
                        }
                    }
                    Err(_) => stats.err(),
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// One connection: up to `burst` keep-alive requests.
async fn h1_keepalive_burst(target: SocketAddr, seed: u64, burst: usize) -> anyhow::Result<usize> {
    let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(target)).await??;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    let driver = tokio::spawn(conn);
    let mut served = 0usize;
    for i in 0..burst {
        let body = body_for(seed.wrapping_add(i as u64));
        let method = if i % 2 == 0 { "GET" } else { "POST" };
        let req = Request::builder()
            .method(method)
            .uri("/")
            .header("host", "localhost")
            .body(body)?;
        match tokio::time::timeout(Duration::from_secs(10), sender.send_request(req)).await {
            Ok(Ok(resp)) => {
                let _ = resp.into_body().collect().await;
                served += 1;
            }
            _ => break,
        }
    }
    drop(sender);
    driver.abort();
    Ok(served)
}

// ── WebSocket-over-HTTP/1.1 load (S27 sc8_ws_h1) ──────────────────────────────
//
// The WS relay is a long-lived opaque tunnel, so the soak-class risk is a connection/fd/memory
// LEAK under churn (the F-S20-2 class). Two drivers split that: `run_ws_h1_load` holds tunnels
// open (the live-connection bound must plateau) and `run_ws_h1_churn` cycles open→close (the
// RECLAIM probe — an unreleased fd/slot/relay task ratchets up across cycles).

/// The outcome of waiting for one echo frame on a WS tunnel. A byte mismatch is a relay
/// integrity DEFECT; a close/read-timeout is only a lifecycle event (the worker reconnects).
enum WsEcho {
    Bytes(Vec<u8>),
    Closed,
}

/// One sustained WS connection: handshake, then loop send→echo-verify until the cancel fires or an error.
async fn ws_h1_echo_loop(
    target: SocketAddr,
    seed: u64,
    cancel: &CancellationToken,
    stats: &LoadStats,
) -> anyhow::Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let url = format!("ws://{target}/soak");
    let (mut ws, _resp) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(url),
    )
    .await??;

    let mut i = seed;
    while !cancel.is_cancelled() {
        i = i.wrapping_add(1);
        let len = BODY_SIZES[(i as usize) % BODY_SIZES.len()].max(1);
        let payload: Vec<u8> = (0..len).map(|k| ((k as u64 + i) % 251) as u8).collect();
        if tokio::time::timeout(
            Duration::from_secs(5),
            ws.send(Message::Binary(payload.clone().into())),
        )
        .await
        .is_err()
        {
            anyhow::bail!("ws send timeout");
        }
        let echoed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Binary(b))) => return WsEcho::Bytes(b.to_vec()),
                    Some(Ok(Message::Close(_))) | None => return WsEcho::Closed,
                    Some(Ok(_)) => continue, // Pong / Text — keep waiting
                    Some(Err(_)) => return WsEcho::Closed,
                }
            }
        })
        .await?;
        match echoed {
            WsEcho::Bytes(b) if b == payload => stats.ok(),
            // A genuine byte mismatch is a RELAY DEFECT — count it + propagate.
            WsEcho::Bytes(_) => {
                stats.err();
                anyhow::bail!("ws ECHO MISMATCH (relay integrity defect)");
            }
            // A clean close mid-loop is a connection-LIFECYCLE event, not an echo-integrity
            // failure: it does NOT increment `err`. (Identical disposition to the H2 path.)
            WsEcho::Closed => return Ok(()),
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = ws.close(None).await;
    Ok(())
}

/// Sustained WS-over-H1 load: `concurrency` LONG-LIVED clients, each looping a bidirectional echo until the cancel fires.
pub async fn run_ws_h1_load(
    target: SocketAddr,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        workers.push(tokio::spawn(async move {
            let mut seed = (w as u64).wrapping_mul(7);
            while !cancel.is_cancelled() {
                seed = seed.wrapping_add(101);
                if let Err(e) = ws_h1_echo_loop(target, seed, &cancel, &stats).await {
                    if std::env::var("WS_DEBUG").is_ok() {
                        eprintln!("[ws_h1_load err] {e}");
                    }
                    stats.err();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// One open→echo→close WS cycle: handshake, run `frames` echo round-trips, then cleanly Close.
async fn ws_h1_open_close_cycle(
    target: SocketAddr,
    seed: u64,
    frames: usize,
) -> anyhow::Result<usize> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let url = format!("ws://{target}/soak");
    let (mut ws, _resp) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(url),
    )
    .await??;

    let mut served = 0usize;
    for f in 0..frames {
        let len = BODY_SIZES[((seed as usize).wrapping_add(f)) % BODY_SIZES.len()].max(1);
        let payload: Vec<u8> = (0..len).map(|k| ((k as u64 + seed) % 251) as u8).collect();
        tokio::time::timeout(
            Duration::from_secs(5),
            ws.send(Message::Binary(payload.clone().into())),
        )
        .await??;
        let echoed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Binary(b))) => return Some(b),
                    Some(Ok(Message::Close(_))) | None => return None,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => return None,
                }
            }
        })
        .await?;
        match echoed {
            Some(b) if b == payload => served += 1,
            _ => anyhow::bail!("ws churn echo mismatch / early close"),
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(3), ws.close(None)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(_)) = ws.next().await {}
    })
    .await;
    Ok(served)
}

/// WS-over-H1 open/close CHURN: `concurrency` workers each repeatedly open a WS, run a short echo burst, then cleanly close — exercising connection + relay RECLAIM (the F-S20-2 leak-class probe).
pub async fn run_ws_h1_churn(
    target: SocketAddr,
    concurrency: usize,
    frames_per_cycle: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        workers.push(tokio::spawn(async move {
            let mut seed = (w as u64).wrapping_mul(13).wrapping_add(1);
            while !cancel.is_cancelled() {
                seed = seed.wrapping_add(53);
                match ws_h1_open_close_cycle(target, seed, frames_per_cycle).await {
                    Ok(n) => {
                        for _ in 0..n {
                            stats.ok();
                        }
                    }
                    Err(e) => {
                        if std::env::var("WS_DEBUG").is_ok() {
                            eprintln!("[ws_h1_churn err] {e}");
                        }
                        stats.err();
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

// ── WebSocket-over-HTTP/2 load (S27 sc8b_ws_h2, RFC 8441 extended CONNECT) ─────
//
// Same leak-class question as sc8_ws_h1, over the RFC 8441 path. The listener needs
// `with_h2_extended_connect(true)` (CF-S27-2, off by default).
//
// CRITICAL (F-S27-2): this client READS NORMALLY — it drains inbound DATA and releases
// flow-control capacity. A NON-reading client would exercise the gated H2 unbounded-buffer
// DoS instead, which is NOT what this soak proves.

/// Bridges an open H2 stream (`SendStream<Bytes>` request-body direction + `RecvStream` response-body direction) into the `AsyncRead + AsyncWrite` surface `tokio_tungstenite` requires.
struct H2StreamAdapter {
    send: h2::SendStream<Bytes>,
    recv: h2::RecvStream,
    leftover: Bytes,
}

impl tokio::io::AsyncRead for H2StreamAdapter {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        if !self.leftover.is_empty() {
            let n = self.leftover.len().min(buf.remaining());
            let chunk = self.leftover.split_to(n);
            buf.put_slice(&chunk);
            return Poll::Ready(Ok(()));
        }
        match self.recv.poll_data(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(Ok(())), // clean EOF
            Poll::Ready(Some(Ok(mut data))) => {
                // Release the whole frame back to the window — the normal-reading contract
                // that keeps the relay bounded (NOT the F-S27-2 path).
                let len = data.len();
                let _ = self.recv.flow_control().release_capacity(len);
                let n = len.min(buf.remaining());
                buf.put_slice(data.get(..n).unwrap_or(&[]));
                bytes::Buf::advance(&mut data, n);
                self.leftover = data;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Err(std::io::Error::other(format!("h2 recv: {e}"))))
            }
        }
    }
}

impl tokio::io::AsyncWrite for H2StreamAdapter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::task::Poll;
        self.send.reserve_capacity(buf.len());
        match self.send.poll_capacity(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(Err(std::io::Error::other(
                "h2 send stream closed before capacity",
            ))),
            Poll::Ready(Some(Ok(cap))) => {
                let n = cap.min(buf.len());
                if n == 0 {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                let chunk = Bytes::copy_from_slice(buf.get(..n).unwrap_or(&[]));
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

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.send.send_data(Bytes::new(), true) {
            Ok(()) => std::task::Poll::Ready(Ok(())),
            Err(e) => {
                std::task::Poll::Ready(Err(std::io::Error::other(format!("h2 shutdown: {e}"))))
            }
        }
    }
}

/// Open the gateway TLS+H2 connection, send an RFC 8441 extended CONNECT for.
async fn open_ws_over_h2(
    tls: &tokio_rustls::TlsConnector,
    target: SocketAddr,
    sni: &str,
) -> anyhow::Result<(
    tokio_tungstenite::WebSocketStream<H2StreamAdapter>,
    tokio::task::JoinHandle<()>,
)> {
    let tcp = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(target)).await??;
    let server_name = rustls_pki_types::ServerName::try_from(sni.to_string())?;
    let tls_stream =
        tokio::time::timeout(Duration::from_secs(5), tls.connect(server_name, tcp)).await??;

    let (h2c, conn) =
        tokio::time::timeout(Duration::from_secs(5), h2::client::handshake(tls_stream)).await??;
    let driver = tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2c = tokio::time::timeout(Duration::from_secs(5), h2c.ready()).await??;

    let uri: http::Uri = format!("https://{sni}/soak").parse()?;
    let mut req = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(uri)
        .body(())?;
    req.extensions_mut()
        .insert(h2::ext::Protocol::from_static("websocket"));

    let (resp_fut, send_stream) = h2c
        .send_request(req, false)
        .map_err(|e| anyhow::anyhow!("send extended CONNECT: {e}"))?;
    let resp = tokio::time::timeout(Duration::from_secs(5), resp_fut).await??;
    if resp.status() != http::StatusCode::OK {
        driver.abort();
        anyhow::bail!(
            "extended CONNECT must yield 200 (the gateway accepts the WS tunnel); got {}",
            resp.status()
        );
    }
    let recv_stream = resp.into_body();
    let adapter = H2StreamAdapter {
        send: send_stream,
        recv: recv_stream,
        leftover: Bytes::new(),
    };
    let ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
        adapter,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    Ok((ws, driver))
}

/// One WS-over-H2 open→echo→close cycle: extended CONNECT → 200 → `frames` byte-verified bidi echoes → clean Close.
async fn ws_h2_open_close_cycle(
    tls: &tokio_rustls::TlsConnector,
    target: SocketAddr,
    sni: &str,
    seed: u64,
    frames: usize,
) -> anyhow::Result<usize> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut ws, driver) = open_ws_over_h2(tls, target, sni).await?;
    let mut served = 0usize;
    for f in 0..frames {
        let len = BODY_SIZES[((seed as usize).wrapping_add(f)) % BODY_SIZES.len()].max(1);
        let payload: Vec<u8> = (0..len).map(|k| ((k as u64 + seed) % 251) as u8).collect();
        tokio::time::timeout(
            Duration::from_secs(5),
            ws.send(Message::Binary(payload.clone().into())),
        )
        .await??;
        let echoed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Binary(b))) => return Some(b),
                    Some(Ok(Message::Close(_))) | None => return None,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => return None,
                }
            }
        })
        .await?;
        match echoed {
            Some(b) if b == payload => served += 1,
            _ => {
                driver.abort();
                anyhow::bail!("ws-over-h2 echo mismatch / early close");
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(3), ws.close(None)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(_)) = ws.next().await {}
    })
    .await;
    driver.abort();
    Ok(served)
}

/// One sustained WS-over-H2 connection: extended CONNECT, then loop send→echo-verify until the cancel fires.
async fn ws_h2_echo_loop(
    tls: &tokio_rustls::TlsConnector,
    target: SocketAddr,
    sni: &str,
    seed: u64,
    cancel: &CancellationToken,
    stats: &LoadStats,
) -> anyhow::Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut ws, driver) = open_ws_over_h2(tls, target, sni).await?;
    let mut i = seed;
    while !cancel.is_cancelled() {
        i = i.wrapping_add(1);
        let len = BODY_SIZES[(i as usize) % BODY_SIZES.len()].max(1);
        let payload: Vec<u8> = (0..len).map(|k| ((k as u64 + i) % 251) as u8).collect();
        if tokio::time::timeout(
            Duration::from_secs(5),
            ws.send(Message::Binary(payload.clone().into())),
        )
        .await
        .is_err()
        {
            driver.abort();
            anyhow::bail!("ws-over-h2 send timeout");
        }
        let echoed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Binary(b))) => return WsEcho::Bytes(b.to_vec()),
                    Some(Ok(Message::Close(_))) | None => return WsEcho::Closed,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => return WsEcho::Closed,
                }
            }
        })
        .await?;
        match echoed {
            WsEcho::Bytes(b) if b == payload => stats.ok(),
            WsEcho::Bytes(_) => {
                stats.err();
                driver.abort();
                anyhow::bail!("ws-over-h2 ECHO MISMATCH (relay integrity defect)");
            }
            WsEcho::Closed => {
                driver.abort();
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = ws.close(None).await;
    driver.abort();
    Ok(())
}

/// Sustained WS-over-H2 load: `concurrency` LONG-LIVED clients each looping a byte-verified bidirectional echo over an RFC 8441 tunnel until cancel.
pub async fn run_ws_h2_load(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let tls = match h2_tls_connector(&ca_path) {
        Ok(t) => t,
        Err(_) => {
            stats.err();
            return;
        }
    };
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let tls = tls.clone();
        let sni = sni.clone();
        workers.push(tokio::spawn(async move {
            let mut seed = (w as u64).wrapping_mul(7);
            while !cancel.is_cancelled() {
                seed = seed.wrapping_add(101);
                if let Err(e) = ws_h2_echo_loop(&tls, target, &sni, seed, &cancel, &stats).await {
                    if std::env::var("WS_DEBUG").is_ok() {
                        eprintln!("[ws_h2_load err] {e}");
                    }
                    stats.err();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// WS-over-H2 open/close CHURN: `concurrency` workers each repeatedly open a WS.
pub async fn run_ws_h2_churn(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    frames_per_cycle: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let tls = match h2_tls_connector(&ca_path) {
        Ok(t) => t,
        Err(_) => {
            stats.err();
            return;
        }
    };
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let tls = tls.clone();
        let sni = sni.clone();
        workers.push(tokio::spawn(async move {
            let mut seed = (w as u64).wrapping_mul(13).wrapping_add(1);
            while !cancel.is_cancelled() {
                seed = seed.wrapping_add(53);
                match ws_h2_open_close_cycle(&tls, target, &sni, seed, frames_per_cycle).await {
                    Ok(n) => {
                        for _ in 0..n {
                            stats.ok();
                        }
                    }
                    Err(e) => {
                        if std::env::var("WS_DEBUG").is_ok() {
                            eprintln!("[ws_h2_churn err] {e}");
                        }
                        stats.err();
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// H2-over-TLS load (front is `h1s`, ALPN selects h2).
pub async fn run_h2_load(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let tls = match h2_tls_connector(&ca_path) {
        Ok(t) => t,
        Err(_) => {
            stats.err();
            return;
        }
    };
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let tls = tls.clone();
        let sni = sni.clone();
        workers.push(tokio::spawn(async move {
            let mut iter = w as u64;
            while !cancel.is_cancelled() {
                iter += 1;
                match h2_stream_batch(&tls, target, &sni, iter, 8).await {
                    Ok(n) => {
                        for _ in 0..n {
                            stats.ok();
                        }
                    }
                    Err(e) => {
                        if std::env::var("H2_DEBUG").is_ok() {
                            eprintln!("[h2_batch err] {e}");
                        }
                        stats.err();
                    }
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// Accept-any server-cert verifier for the loopback LOAD client. The gateway's loopback certs
/// are `is_ca=true` (required so quiche accepts them as their own CA), which rustls rejects as
/// an end-entity leaf. This is load-client-only code and never ships on an operator path.
#[derive(Debug)]
struct AcceptAnyServerCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Build a rustls TLS connector for the loopback load client (accept-any cert, ALPN `h2`).
pub fn h2_tls_connector(_ca_path: &std::path::Path) -> anyhow::Result<tokio_rustls::TlsConnector> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert(provider)))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Ok(tokio_rustls::TlsConnector::from(Arc::new(cfg)))
}

async fn h2_stream_batch(
    tls: &tokio_rustls::TlsConnector,
    target: SocketAddr,
    sni: &str,
    seed: u64,
    batch: usize,
) -> anyhow::Result<usize> {
    let tcp = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(target)).await??;
    let server_name = rustls_pki_types::ServerName::try_from(sni.to_string())?;
    let tls_stream =
        tokio::time::timeout(Duration::from_secs(5), tls.connect(server_name, tcp)).await??;
    let (mut sender, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls_stream))
            .await?;
    let driver = tokio::spawn(conn);
    let mut futs = Vec::new();
    for i in 0..batch {
        let body = body_for(seed.wrapping_add(i as u64));
        let method = if i % 2 == 0 { "GET" } else { "POST" };
        // hyper's H2 client requires ABSOLUTE-form URIs (to populate :scheme/:authority).
        let req = Request::builder()
            .method(method)
            .uri(format!("https://{sni}/"))
            .body(body)?;
        futs.push(sender.send_request(req));
    }
    let mut served = 0usize;
    for f in futs {
        if let Ok(Ok(resp)) = tokio::time::timeout(Duration::from_secs(10), f).await {
            let _ = resp.into_body().collect().await;
            served += 1;
        }
    }
    drop(sender);
    driver.abort();
    Ok(served)
}

/// QUIC load for Mode A (passthrough; `target` is the gateway passthrough bind,.
#[allow(clippy::too_many_arguments)]
pub async fn run_quic_load(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    streams_per_conn: usize,
    payload_len: usize,
    datagrams_per_conn: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let sni = sni.clone();
        let ca_path = ca_path.clone();
        workers.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                match quic_session(
                    target,
                    &sni,
                    &ca_path,
                    streams_per_conn,
                    payload_len,
                    datagrams_per_conn,
                )
                .await
                {
                    Ok(()) => stats.ok(),
                    Err(e) => {
                        if std::env::var("QUIC_DEBUG").is_ok() {
                            eprintln!("[quic_session err] {e}");
                        }
                        stats.err();
                    }
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

fn quic_client_config(ca_path: &std::path::Path) -> anyhow::Result<quiche::Config> {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)
        .map_err(|e| anyhow::anyhow!("quiche config: {e:?}"))?;
    cfg.set_application_protos(&[b"h3", b"h3-29"])
        .map_err(|e| anyhow::anyhow!("alpn: {e:?}"))?;
    let ca = ca_path.to_str().ok_or_else(|| anyhow::anyhow!("ca path"))?;
    cfg.load_verify_locations_from_file(ca)
        .map_err(|e| anyhow::anyhow!("load ca: {e:?}"))?;
    cfg.verify_peer(true);
    cfg.set_max_idle_timeout(10_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(8 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    cfg.set_initial_max_stream_data_uni(256 * 1024);
    cfg.set_initial_max_streams_bidi(128);
    cfg.set_initial_max_streams_uni(128);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    Ok(cfg)
}

/// One full client connection lifecycle: handshake → N echo-verified bidi streams → optional datagram flood → close.
async fn quic_session(
    target: SocketAddr,
    sni: &str,
    ca_path: &std::path::Path,
    streams_per_conn: usize,
    payload_len: usize,
    datagrams_per_conn: usize,
) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(("127.0.0.1", 0)).await?;
    let local = socket.local_addr()?;
    let mut cfg = quic_client_config(ca_path)?;
    let scid = random_cid();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(Some(sni), &scid_ref, local, target, &mut cfg)
        .map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;

    let mut out = vec![0u8; MAX_UDP];
    let mut inb = vec![0u8; MAX_UDP];
    let mut rd = vec![0u8; MAX_UDP];

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    flush(&mut conn, &socket, &mut out).await?;
    while !conn.is_established() {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("handshake timeout");
        }
        recv_one(
            &mut conn,
            &socket,
            local,
            &mut inb,
            Duration::from_millis(50),
        )
        .await;
        flush(&mut conn, &socket, &mut out).await?;
        if conn.is_closed() {
            anyhow::bail!("closed during handshake");
        }
    }

    let payload: Vec<u8> = (0..payload_len)
        .map(|i| ((i * 31 + 7) % 256) as u8)
        .collect();
    // `expecting[sid]` tracks echo bytes received; `send_off[sid]` tracks payload bytes the
    // local quiche ACCEPTED on the send side (FIN applies only on the call that accepts the
    // final bytes).
    //
    // F-S20-1 (S21): `stream_send` is bounded by the connection's cwnd-aware SEND CAPACITY and
    // may accept only a PREFIX — the initial ~13.5 KB window is shared across streams. The
    // client MUST re-send the remainder as capacity frees; calling `stream_send` once is what
    // made S20 misread a load-client truncation as a gateway relay stall.
    let mut expecting: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut send_off: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for s in 0..streams_per_conn {
        let sid = (s as u64) * 4; // client-initiated bidi stream ids: 0,4,8,…
        expecting.insert(sid, 0);
        send_off.insert(sid, 0);
    }
    for _ in 0..datagrams_per_conn {
        let _ = conn.dgram_send(&payload[..payload_len.min(1024)]);
    }
    flush(&mut conn, &socket, &mut out).await?;

    let relay_deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    while !expecting.is_empty() {
        if tokio::time::Instant::now() > relay_deadline || conn.is_closed() {
            // Diagnostic (F-S20-1): `sent<want` is a load-client send bug; `sent==want` with
            // `got<want` is a genuine relay/echo tail loss.
            let mut left: Vec<(u64, usize)> = expecting.iter().map(|(k, v)| (*k, *v)).collect();
            left.sort_unstable();
            let detail: Vec<String> = left
                .iter()
                .map(|(sid, got)| {
                    let sent = send_off.get(sid).copied().unwrap_or(0);
                    format!("sid{sid}=got{got}/sent{sent}/want{payload_len}")
                })
                .collect();
            anyhow::bail!(
                "relay timeout / closed (streams left: {} [{}]); closed={}",
                expecting.len(),
                detail.join(" "),
                conn.is_closed()
            );
        }
        // Keep pushing each stream's remaining payload + FIN as cwnd frees up.
        let mut wrote = false;
        for s in 0..streams_per_conn {
            let sid = (s as u64) * 4;
            let off = *send_off.get(&sid).unwrap_or(&0);
            if off >= payload.len() {
                continue; // fully sent (payload + FIN)
            }
            match conn.stream_send(sid, &payload[off..], true) {
                Ok(n) => {
                    send_off.insert(sid, off + n);
                    if n > 0 {
                        wrote = true;
                    }
                }
                Err(quiche::Error::Done) | Err(quiche::Error::StreamLimit) => {}
                Err(e) => anyhow::bail!("stream_send sid={sid}: {e:?}"),
            }
        }
        if wrote {
            flush(&mut conn, &socket, &mut out).await?;
        }
        recv_one(
            &mut conn,
            &socket,
            local,
            &mut inb,
            Duration::from_millis(20),
        )
        .await;
        let readable: Vec<u64> = conn.readable().collect();
        for sid in readable {
            loop {
                match conn.stream_recv(sid, &mut rd) {
                    Ok((n, fin)) => {
                        if let Some(got) = expecting.get_mut(&sid) {
                            *got += n;
                        }
                        if fin {
                            if let Some(got) = expecting.get(&sid).copied() {
                                if got != payload.len() {
                                    anyhow::bail!(
                                        "echo length mismatch sid={sid}: got {got} want {}",
                                        payload.len()
                                    );
                                }
                            }
                            expecting.remove(&sid);
                            break;
                        }
                        if n == 0 {
                            break;
                        }
                    }
                    Err(quiche::Error::Done) => break,
                    Err(e) => anyhow::bail!("stream_recv sid={sid}: {e:?}"),
                }
            }
        }
        while conn.dgram_recv(&mut rd).is_ok() {}
        flush(&mut conn, &socket, &mut out).await?;
    }

    let _ = conn.close(true, 0x0, b"done");
    let _ = flush(&mut conn, &socket, &mut out).await;
    Ok(())
}

// ── H3-terminate load (S26 / INC-D) ──────────────────────────────────────────
//
// A REAL `quiche::h3::Connection` client against the gateway's H3-terminate front.
// `with_transport` opens the control + QPACK uni streams and sends SETTINGS, so this drives
// the ingress as a conformant peer would — raw opaque bytes would never reach the H3 layer.
//
// Against a backend-less front there are exactly two observable outcomes and BOTH are
// asserted (non-vacuous): a bad `:authority` MUST read back the inline-400 (a true
// request→response round-trip), and a valid `:authority` MUST be silently dropped — no
// response and no stream reset — within a short drop-window. A 2xx or a 400 on the valid
// class is the failure.

/// How long the bad-authority class waits for the inline-400 round-trip before declaring the ingress hung (the 400 is generated locally + arrives fast).
const H3_RESP_BUDGET: Duration = Duration::from_secs(5);
/// How long the valid-authority class polls for a (never-arriving) response before concluding the EXPECTED silent no-backend drop.
const H3_DROP_WINDOW: Duration = Duration::from_millis(600);

/// The per-request outcome the client verified for its class (non-vacuous).
enum H3Outcome {
    /// Bad-authority class: read the inline-400 (`:status 400` + body) — a true request→response round-trip.
    Verified400,
    /// Valid-authority class: passed the validator then was silently dropped (no backend) within the drop-window, with no 2xx and no 400.
    BoundedDrop,
}

/// Sustained H3-terminate load.
pub async fn run_h3_load(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    requests_per_conn: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let sni = sni.clone();
        let ca_path = ca_path.clone();
        workers.push(tokio::spawn(async move {
            let mut iter = w as u64;
            while !cancel.is_cancelled() {
                iter = iter.wrapping_add(1);
                match h3_session(target, &sni, &ca_path, requests_per_conn, iter).await {
                    Ok((ok, err)) => {
                        for _ in 0..ok {
                            stats.ok();
                        }
                        for _ in 0..err {
                            stats.err();
                        }
                    }
                    Err(e) => {
                        if std::env::var("H3_DEBUG").is_ok() {
                            eprintln!("[h3_session err] {e}");
                        }
                        stats.err();
                    }
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// H3 RST/STOP_SENDING flood — the F-MD-4 chaos injector for the H3-terminate front. Each
/// worker rapidly opens request streams and alternately `RESET_STREAM`s them (the gateway's
/// request-side `StreamReset` arm) and `STOP_SENDING`s them (the `StreamStopped` arm).
///
/// The bound under test (R8): stream table + reset accounting + response-producer tasks stay
/// bounded — the H3 analogue of the H2 rapid-reset (CVE-2023-44487) injector. It lives here,
/// not in `chaos`, because it is a property of the QUIC/H3 session.
pub async fn run_h3_reset_flood(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let sni = sni.clone();
        let ca_path = ca_path.clone();
        workers.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                match h3_reset_burst(target, &sni, &ca_path, 200, &cancel).await {
                    Ok(n) => {
                        for _ in 0..n {
                            stats.ok();
                        }
                    }
                    Err(_) => {
                        stats.err();
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// One reset-flood connection: handshake → `quiche::h3` client → up to `burst` open-then-reset request streams.
async fn h3_reset_burst(
    target: SocketAddr,
    sni: &str,
    ca_path: &std::path::Path,
    burst: usize,
    cancel: &CancellationToken,
) -> anyhow::Result<usize> {
    let socket = UdpSocket::bind(("127.0.0.1", 0)).await?;
    let local = socket.local_addr()?;
    let mut cfg = quic_client_config(ca_path)?;
    let scid = random_cid();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(Some(sni), &scid_ref, local, target, &mut cfg)
        .map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;

    let mut out = vec![0u8; MAX_UDP];
    let mut inb = vec![0u8; MAX_UDP];

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    flush(&mut conn, &socket, &mut out).await?;
    while !conn.is_established() {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("handshake timeout");
        }
        recv_one(
            &mut conn,
            &socket,
            local,
            &mut inb,
            Duration::from_millis(50),
        )
        .await;
        flush(&mut conn, &socket, &mut out).await?;
        if conn.is_closed() {
            anyhow::bail!("closed during handshake");
        }
    }

    let h3cfg = quiche::h3::Config::new().map_err(|e| anyhow::anyhow!("h3::Config: {e:?}"))?;
    let mut h3 = quiche::h3::Connection::with_transport(&mut conn, &h3cfg)
        .map_err(|e| anyhow::anyhow!("h3 with_transport: {e:?}"))?;
    flush(&mut conn, &socket, &mut out).await?;

    let headers = [
        quiche::h3::Header::new(b":method", b"POST"),
        quiche::h3::Header::new(b":scheme", b"https"),
        quiche::h3::Header::new(b":path", b"/soak"),
        quiche::h3::Header::new(b":authority", b"example.test:443"),
        quiche::h3::Header::new(b"content-length", b"65536"),
    ];

    let mut tore_down = 0usize;
    for i in 0..burst {
        if cancel.is_cancelled() || conn.is_closed() {
            break;
        }
        let sid = match h3.send_request(&mut conn, &headers, false) {
            Ok(id) => id,
            // Stream-limit / window pressure: flush + drain so MAX_STREAMS advances.
            Err(quiche::h3::Error::StreamBlocked) | Err(quiche::h3::Error::Done) => {
                flush(&mut conn, &socket, &mut out).await?;
                recv_one(
                    &mut conn,
                    &socket,
                    local,
                    &mut inb,
                    Duration::from_millis(10),
                )
                .await;
                while h3.poll(&mut conn).is_ok() {}
                continue;
            }
            Err(_) => break,
        };
        // Alternate the F-MD-4 trigger: RESET_STREAM (peer-reset the stream the gateway is
        // READING) vs STOP_SENDING (peer-stop the stream it would WRITE the response on).
        // H3_REQUEST_CANCELLED = 0x10C.
        if i % 2 == 0 {
            let _ = conn.stream_shutdown(sid, quiche::Shutdown::Write, 0x10C);
        } else {
            let _ = conn.stream_shutdown(sid, quiche::Shutdown::Read, 0x10C);
        }
        tore_down += 1;
        // quiche needs a send() after a stream_shutdown or the frames never reach the peer.
        flush(&mut conn, &socket, &mut out).await?;
        if i % 16 == 0 {
            recv_one(
                &mut conn,
                &socket,
                local,
                &mut inb,
                Duration::from_millis(5),
            )
            .await;
            while h3.poll(&mut conn).is_ok() {}
        }
    }

    let _ = conn.close(true, 0x100, b"flood-done");
    let _ = flush(&mut conn, &socket, &mut out).await;
    Ok(tore_down)
}

/// One H3 connection: handshake → `quiche::h3` client → `requests` mixed requests, each
/// verified per its class. Returns `(ok, err)`; an `Err` is a transport-level failure.
async fn h3_session(
    target: SocketAddr,
    sni: &str,
    ca_path: &std::path::Path,
    requests: usize,
    seed: u64,
) -> anyhow::Result<(usize, usize)> {
    let socket = UdpSocket::bind(("127.0.0.1", 0)).await?;
    let local = socket.local_addr()?;
    let mut cfg = quic_client_config(ca_path)?;
    let scid = random_cid();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(Some(sni), &scid_ref, local, target, &mut cfg)
        .map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;

    let mut out = vec![0u8; MAX_UDP];
    let mut inb = vec![0u8; MAX_UDP];

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    flush(&mut conn, &socket, &mut out).await?;
    while !conn.is_established() {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("handshake timeout");
        }
        recv_one(
            &mut conn,
            &socket,
            local,
            &mut inb,
            Duration::from_millis(50),
        )
        .await;
        flush(&mut conn, &socket, &mut out).await?;
        if conn.is_closed() {
            anyhow::bail!("closed during handshake");
        }
    }

    let h3cfg = quiche::h3::Config::new().map_err(|e| anyhow::anyhow!("h3::Config: {e:?}"))?;
    let mut h3 = quiche::h3::Connection::with_transport(&mut conn, &h3cfg)
        .map_err(|e| anyhow::anyhow!("h3 with_transport: {e:?}"))?;
    flush(&mut conn, &socket, &mut out).await?;

    let mut ok = 0usize;
    let mut err = 0usize;
    for i in 0..requests {
        if conn.is_closed() {
            break;
        }
        // Alternate classes so every connection drives BOTH the inline-400 egress and the
        // no-backend drop. Even = bad authority (→400), odd = valid authority (→drop).
        let bad_authority = i % 2 == 0;
        let body_len = BODY_SIZES[((seed as usize).wrapping_add(i)) % BODY_SIZES.len()];
        // Per-request outcome is ISOLATED: a class violation is `err` but does NOT abort the
        // batch. Only a transport-level failure propagates.
        match h3_one_request(
            &mut conn,
            &mut h3,
            &socket,
            local,
            &mut out,
            &mut inb,
            bad_authority,
            body_len,
        )
        .await
        {
            Ok(_outcome) => ok += 1,
            Err(e) => {
                if std::env::var("H3_DEBUG").is_ok() {
                    eprintln!("[h3_request err bad_authority={bad_authority}] {e}");
                }
                err += 1;
            }
        }
    }

    let _ = conn.close(true, 0x100, b"done"); // H3_NO_ERROR
    let _ = flush(&mut conn, &socket, &mut out).await;
    Ok((ok, err))
}

/// Issue ONE H3 request and verify the class-specific outcome end-to-end.
///
/// Bad-authority MUST read the inline 400 + body within `H3_RESP_BUDGET`. Valid-authority is
/// expected to be silently dropped, so a 2xx, a 400, or a connection-level close mid-poll are
/// all failures — the ingress must not tear the connection down for one dropped request.
#[allow(clippy::too_many_arguments)]
async fn h3_one_request(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    socket: &UdpSocket,
    local: SocketAddr,
    out: &mut [u8],
    inb: &mut [u8],
    bad_authority: bool,
    body_len: usize,
) -> anyhow::Result<H3Outcome> {
    // A comma in :authority is the canonical reject case (ROUND8-L7-16 / the HAProxy comma
    // class); a clean host:port is the valid case.
    let authority = if bad_authority {
        "victim.example,attacker.example"
    } else {
        "example.test:443"
    };
    let method = if body_len == 0 { "GET" } else { "POST" };
    let mut headers = vec![
        quiche::h3::Header::new(b":method", method.as_bytes()),
        quiche::h3::Header::new(b":scheme", b"https"),
        quiche::h3::Header::new(b":path", b"/soak"),
        quiche::h3::Header::new(b":authority", authority.as_bytes()),
    ];
    let cl = body_len.to_string();
    if body_len > 0 {
        headers.push(quiche::h3::Header::new(b"content-length", cl.as_bytes()));
    }

    let bodyless = body_len == 0;
    // `StreamBlocked` means the peer's bidi MAX_STREAMS grant is exhausted — normal
    // flow-control under churn, NOT a gateway fault (the S21 lesson: a load client must HONOR
    // flow control). Pump, then retry; only a persistent block is a failure.
    let open_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    // `loop { break id }` rather than `Option` + `.expect()`: infallible-by-construction still
    // trips the panic-freedom deny lint (S34).
    let stream_id = loop {
        match h3.send_request(conn, &headers, bodyless) {
            Ok(id) => break id,
            Err(quiche::h3::Error::StreamBlocked) | Err(quiche::h3::Error::Done) => {
                if tokio::time::Instant::now() > open_deadline {
                    anyhow::bail!("send_request stayed StreamBlocked past the open budget");
                }
                flush(conn, socket, out).await?;
                recv_one(conn, socket, local, inb, Duration::from_millis(20)).await;
                while h3.poll(conn).is_ok() {} // drain so MAX_STREAMS re-arms
                if conn.is_closed() {
                    anyhow::bail!("connection closed before a request stream could open");
                }
            }
            Err(e) => anyhow::bail!("send_request: {e:?}"),
        }
    };
    flush(conn, socket, out).await?;

    let mut body_sent = 0usize;
    let body: Vec<u8> = (0..body_len).map(|i| ((i * 31 + 7) % 256) as u8).collect();

    let mut status: Option<u16> = None;
    let mut resp_body = 0usize;
    let mut finished = false;
    let mut scratch = [0u8; 16 * 1024];
    // Valid-authority polls only the short drop-window — no response is the expected outcome.
    let budget = if bad_authority {
        H3_RESP_BUDGET
    } else {
        H3_DROP_WINDOW
    };
    let req_deadline = tokio::time::Instant::now() + budget;

    while !finished && tokio::time::Instant::now() < req_deadline {
        if body_sent < body.len() {
            match h3.send_body(conn, stream_id, body.get(body_sent..).unwrap_or(&[]), true) {
                Ok(n) => body_sent += n,
                Err(quiche::h3::Error::Done) => {}
                // A reset on the no-backend drop path is an expected teardown.
                Err(_) => body_sent = body.len(),
            }
        }
        flush(conn, socket, out).await?;
        recv_one(conn, socket, local, inb, Duration::from_millis(20)).await;

        loop {
            match h3.poll(conn) {
                Ok((sid, quiche::h3::Event::Headers { list, .. })) if sid == stream_id => {
                    for h in &list {
                        use quiche::h3::NameValue;
                        if h.name() == b":status" {
                            status = std::str::from_utf8(h.value())
                                .ok()
                                .and_then(|s| s.parse().ok());
                        }
                    }
                }
                Ok((sid, quiche::h3::Event::Data)) if sid == stream_id => {
                    while let Ok(n) = h3.recv_body(conn, stream_id, &mut scratch) {
                        if n == 0 {
                            break;
                        }
                        resp_body += n;
                    }
                }
                Ok((sid, quiche::h3::Event::Finished)) if sid == stream_id => {
                    finished = true;
                    break;
                }
                Ok((sid, quiche::h3::Event::Reset(_))) if sid == stream_id => {
                    finished = true;
                    break;
                }
                Ok(_) => {} // other streams / GoAway / PriorityUpdate — ignore
                Err(quiche::h3::Error::Done) => break,
                Err(e) => {
                    if conn.is_closed() {
                        finished = true;
                        break;
                    }
                    anyhow::bail!("h3.poll: {e:?}");
                }
            }
        }
        if conn.is_closed() {
            break;
        }
    }

    // Class-specific NON-VACUOUS verdict.
    if bad_authority {
        // Inline-400 decoded egress: the client MUST read :status 400 AND the body.
        if status != Some(400) {
            anyhow::bail!(
                "bad-:authority must yield the inline 400 decoded egress within {budget:?}; \
                 got status={status:?} (hang ⇒ the migrated quiche::h3 ingress stalled)"
            );
        }
        if resp_body == 0 {
            anyhow::bail!("inline-400 must carry a body (\"bad request\"); read 0 bytes");
        }
        Ok(H3Outcome::Verified400)
    } else {
        // No-backend silent drop: NO `:status` must arrive. A 2xx is impossible; a 400 means
        // the valid authority was wrongly rejected; a connection close means the ingress tore
        // down the whole conn for one dropped request.
        if let Some(s) = status {
            if (200..300).contains(&s) {
                anyhow::bail!(
                    "valid-:authority unexpectedly got a 2xx ({s}) — the front is \
                     backend-less (F-S26-1), so no upstream response is possible"
                );
            }
            if s == 400 {
                anyhow::bail!(
                    "valid-:authority was rejected 400 — the H3 authority validator over-rejected"
                );
            }
            anyhow::bail!("valid-:authority got an unexpected status {s} on a backend-less front");
        }
        if conn.is_closed() {
            anyhow::bail!(
                "the connection closed while a valid-:authority request was dropped — the \
                 ingress must drop the STREAM, not tear down the connection"
            );
        }
        Ok(H3Outcome::BoundedDrop)
    }
}

async fn flush(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    out: &mut [u8],
) -> anyhow::Result<()> {
    loop {
        match conn.send(out) {
            Ok((n, info)) => {
                socket.send_to(out.get(..n).unwrap_or(&[]), info.to).await?;
            }
            Err(quiche::Error::Done) => break,
            Err(e) => anyhow::bail!("conn.send: {e:?}"),
        }
    }
    Ok(())
}

async fn recv_one(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    local: SocketAddr,
    inb: &mut [u8],
    wait: Duration,
) {
    match tokio::time::timeout(wait, socket.recv_from(inb)).await {
        Ok(Ok((n, from))) => {
            let info = quiche::RecvInfo { from, to: local };
            let slice = inb.get_mut(..n).unwrap_or(&mut []);
            let _ = conn.recv(slice, info);
        }
        _ => {
            conn.on_timeout();
        }
    }
}

fn random_cid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    use ring::rand::SecureRandom;
    let mut cid = [0u8; quiche::MAX_CONN_ID_LEN];
    let _ = ring::rand::SystemRandom::new().fill(&mut cid);
    cid
}

// ── WebSocket-over-HTTP/3 load (S28 sc8c_ws_h3, RFC 9220 extended CONNECT) ─────
//
// Same leak-class question as sc8_ws_h1 (the F-S20-2 class) over the H3/quiche datapath. The
// client frames WS by hand (a raw quiche stream cannot be wrapped in tungstenite), so payloads
// stay < 126 bytes — the 7-bit length form.

/// Encode a masked client WS frame (RFC 6455 §5.2).
#[allow(clippy::indexing_slicing)]
fn ws_mask_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 6);
    f.push(0x80 | opcode); // FIN + opcode
    f.push(0x80 | (payload.len() as u8)); // MASK + 7-bit len (payload < 126)
    let mask = [0x3a_u8, 0x5b, 0x7c, 0x9d];
    f.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        f.push(b ^ mask[i % 4]);
    }
    f
}

/// Parse the first complete WS frame from `buf` → `(opcode, payload, consumed)`.
#[allow(clippy::indexing_slicing)]
fn ws_parse_one(buf: &[u8]) -> Option<(u8, Vec<u8>, usize)> {
    if buf.len() < 2 {
        return None;
    }
    let opcode = buf[0] & 0x0F;
    let masked = buf[1] & 0x80 != 0;
    let len7 = (buf[1] & 0x7F) as usize;
    let mut idx = 2usize;
    let plen = match len7.cmp(&126) {
        std::cmp::Ordering::Less => len7,
        std::cmp::Ordering::Equal => {
            if buf.len() < 4 {
                return None;
            }
            let l = ((buf[2] as usize) << 8) | (buf[3] as usize);
            idx = 4;
            l
        }
        std::cmp::Ordering::Greater => return None,
    };
    let mask = if masked {
        if buf.len() < idx + 4 {
            return None;
        }
        let m = [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]];
        idx += 4;
        Some(m)
    } else {
        None
    };
    if buf.len() < idx + plen {
        return None;
    }
    let mut pl = buf[idx..idx + plen].to_vec();
    if let Some(m) = mask {
        for (i, b) in pl.iter_mut().enumerate() {
            *b ^= m[i % 4];
        }
    }
    Some((opcode, pl, idx + plen))
}

/// Poll the h3 connection, accumulating `:status` and DATA bytes (the tunneled WS frames) for the WS tunnel stream.
fn ws_h3_drain(
    h3: &mut quiche::h3::Connection,
    conn: &mut quiche::Connection,
    status: &mut Option<u16>,
    rx: &mut Vec<u8>,
) {
    use quiche::h3::NameValue;
    loop {
        match h3.poll(conn) {
            Ok((_s, quiche::h3::Event::Headers { list, .. })) => {
                for h in &list {
                    if h.name() == b":status" {
                        *status = std::str::from_utf8(h.value())
                            .ok()
                            .and_then(|s| s.parse().ok());
                    }
                }
            }
            Ok((s, quiche::h3::Event::Data)) => {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = h3.recv_body(conn, s, &mut chunk) {
                    if n == 0 {
                        break;
                    }
                    rx.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                }
            }
            Ok(_) => {}
            Err(quiche::h3::Error::Done) | Err(_) => break,
        }
    }
}

/// One WS-over-H3 session: handshake → extended CONNECT → 200 → echo round-trips → clean
/// Close. `until_cancel` HOLDS the tunnel open (sustained pressure); otherwise it runs
/// `max_frames` then closes (churn / reclaim).
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn ws_h3_echo_session(
    target: SocketAddr,
    sni: &str,
    ca_path: &std::path::Path,
    seed: u64,
    max_frames: usize,
    until_cancel: bool,
    stats: &LoadStats,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(("127.0.0.1", 0)).await?;
    let local = socket.local_addr()?;
    let mut cfg = quic_client_config(ca_path)?;
    let scid = random_cid();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(Some(sni), &scid_ref, local, target, &mut cfg)
        .map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;
    let mut out = vec![0u8; MAX_UDP];
    let mut inb = vec![0u8; MAX_UDP];

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    flush(&mut conn, &socket, &mut out).await?;
    while !conn.is_established() {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("handshake timeout");
        }
        recv_one(
            &mut conn,
            &socket,
            local,
            &mut inb,
            Duration::from_millis(50),
        )
        .await;
        flush(&mut conn, &socket, &mut out).await?;
        if conn.is_closed() {
            anyhow::bail!("closed during handshake");
        }
    }
    let h3cfg = quiche::h3::Config::new().map_err(|e| anyhow::anyhow!("h3::Config: {e:?}"))?;
    let mut h3 = quiche::h3::Connection::with_transport(&mut conn, &h3cfg)
        .map_err(|e| anyhow::anyhow!("h3 with_transport: {e:?}"))?;
    flush(&mut conn, &socket, &mut out).await?;

    let headers = [
        quiche::h3::Header::new(b":method", b"CONNECT"),
        quiche::h3::Header::new(b":protocol", b"websocket"),
        quiche::h3::Header::new(b":scheme", b"https"),
        quiche::h3::Header::new(b":authority", sni.as_bytes()),
        quiche::h3::Header::new(b":path", b"/soak"),
    ];
    let sid = loop {
        match h3.send_request(&mut conn, &headers, false) {
            Ok(s) => break s,
            Err(quiche::h3::Error::StreamBlocked) | Err(quiche::h3::Error::Done) => {
                flush(&mut conn, &socket, &mut out).await?;
                recv_one(
                    &mut conn,
                    &socket,
                    local,
                    &mut inb,
                    Duration::from_millis(10),
                )
                .await;
            }
            Err(e) => anyhow::bail!("send_request: {e:?}"),
        }
        if conn.is_closed() {
            anyhow::bail!("closed before CONNECT");
        }
    };

    let mut status: Option<u16> = None;
    let mut rx: Vec<u8> = Vec::new();
    let st_deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while status.is_none() {
        if tokio::time::Instant::now() > st_deadline {
            anyhow::bail!("no 200 (status timeout)");
        }
        flush(&mut conn, &socket, &mut out).await?;
        recv_one(
            &mut conn,
            &socket,
            local,
            &mut inb,
            Duration::from_millis(20),
        )
        .await;
        ws_h3_drain(&mut h3, &mut conn, &mut status, &mut rx);
        if conn.is_closed() {
            anyhow::bail!("closed before 200");
        }
    }
    if status != Some(200) {
        anyhow::bail!("extended CONNECT status {status:?}");
    }

    let mut f = 0usize;
    while (until_cancel && !cancel.is_cancelled()) || (!until_cancel && f < max_frames) {
        if conn.is_closed() {
            anyhow::bail!("tunnel closed mid-echo");
        }
        let len = 16 + (((seed as usize).wrapping_add(f)) % 80); // 16..96 (< 126)
        let payload: Vec<u8> = (0..len)
            .map(|k| ((k as u64).wrapping_add(seed).wrapping_add(f as u64) % 251) as u8)
            .collect();
        // BINARY (0x2), not Text: a WS Text frame MUST be valid UTF-8 (RFC 6455 §5.6) and
        // tungstenite correctly tears the tunnel down on non-UTF-8 Text.
        let frame = ws_mask_frame(0x2, &payload);

        let send_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match h3.send_body(&mut conn, sid, &frame, false) {
                Ok(n) if n == frame.len() => break,
                Ok(_) | Err(quiche::h3::Error::Done) => {
                    flush(&mut conn, &socket, &mut out).await?;
                    recv_one(
                        &mut conn,
                        &socket,
                        local,
                        &mut inb,
                        Duration::from_millis(5),
                    )
                    .await;
                }
                Err(e) => anyhow::bail!("ws send_body: {e:?}"),
            }
            if tokio::time::Instant::now() > send_deadline {
                anyhow::bail!("ws send timeout");
            }
        }
        flush(&mut conn, &socket, &mut out).await?;

        let echo_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut got = false;
        while !got {
            if tokio::time::Instant::now() > echo_deadline {
                anyhow::bail!("ws echo timeout");
            }
            recv_one(
                &mut conn,
                &socket,
                local,
                &mut inb,
                Duration::from_millis(20),
            )
            .await;
            ws_h3_drain(&mut h3, &mut conn, &mut status, &mut rx);
            while let Some((op, pl, consumed)) = ws_parse_one(&rx) {
                rx.drain(..consumed);
                if op == 0x2 && pl == payload {
                    got = true;
                    stats.ok();
                }
            }
            flush(&mut conn, &socket, &mut out).await?;
            if conn.is_closed() {
                anyhow::bail!("tunnel closed awaiting echo");
            }
        }
        f += 1;
        if until_cancel {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    let close = ws_mask_frame(0x8, &[0x03, 0xE8]);
    let _ = h3.send_body(&mut conn, sid, &close, false);
    let _ = flush(&mut conn, &socket, &mut out).await;
    for _ in 0..6 {
        recv_one(
            &mut conn,
            &socket,
            local,
            &mut inb,
            Duration::from_millis(10),
        )
        .await;
        let _ = flush(&mut conn, &socket, &mut out).await;
    }
    let _ = conn.close(true, 0x100, b"ws-done");
    let _ = flush(&mut conn, &socket, &mut out).await;
    Ok(())
}

/// WS-over-H3 SUSTAINED load: `concurrency` workers each HOLD a WS tunnel open looping bidirectional echoes (held-tunnel pressure).
pub async fn run_ws_h3_load(
    target: SocketAddr,
    sni: String,
    ca: PathBuf,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let (sni, ca, stats, cancel) =
            (sni.clone(), ca.clone(), Arc::clone(&stats), cancel.clone());
        workers.push(tokio::spawn(async move {
            let mut seed = (w as u64).wrapping_mul(17).wrapping_add(3);
            while !cancel.is_cancelled() {
                seed = seed.wrapping_add(101);
                if let Err(e) =
                    ws_h3_echo_session(target, &sni, &ca, seed, usize::MAX, true, &stats, &cancel)
                        .await
                {
                    if std::env::var("WS_DEBUG").is_ok() {
                        eprintln!("[ws_h3_load err] {e}");
                    }
                    stats.err();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// WS-over-H3 open/close CHURN: `concurrency` workers each repeatedly open a WS, run a short echo burst, then cleanly close — the F-S20-2 reclaim probe over H3.
pub async fn run_ws_h3_churn(
    target: SocketAddr,
    sni: String,
    ca: PathBuf,
    concurrency: usize,
    frames_per_cycle: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let (sni, ca, stats, cancel) =
            (sni.clone(), ca.clone(), Arc::clone(&stats), cancel.clone());
        workers.push(tokio::spawn(async move {
            let mut seed = (w as u64).wrapping_mul(29).wrapping_add(7);
            while !cancel.is_cancelled() {
                seed = seed.wrapping_add(53);
                if let Err(e) = ws_h3_echo_session(
                    target,
                    &sni,
                    &ca,
                    seed,
                    frames_per_cycle,
                    false,
                    &stats,
                    &cancel,
                )
                .await
                {
                    if std::env::var("WS_DEBUG").is_ok() {
                        eprintln!("[ws_h3_churn err] {e}");
                    }
                    stats.err();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// Build one gRPC length-prefixed frame: a 1-byte compression flag (0) + a 4-byte big-endian length + the message.
fn frame_grpc(payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + payload.len());
    v.push(0u8);
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// Poll quiche::h3 events for one request stream: capture `:status`, the `grpc-status` trailer (from the trailing HEADERS), the response body bytes, and the FIN (`Event::Finished`).
fn grpc_h3_drain(
    h3: &mut quiche::h3::Connection,
    conn: &mut quiche::Connection,
    sid: u64,
    status: &mut Option<u16>,
    grpc_status: &mut Option<i32>,
    body: &mut Vec<u8>,
    finished: &mut bool,
) {
    use quiche::h3::NameValue;
    loop {
        match h3.poll(conn) {
            Ok((s, quiche::h3::Event::Headers { list, .. })) if s == sid => {
                for h in &list {
                    if h.name() == b":status" {
                        *status = std::str::from_utf8(h.value())
                            .ok()
                            .and_then(|v| v.parse().ok());
                    } else if h.name() == b"grpc-status" {
                        *grpc_status = std::str::from_utf8(h.value())
                            .ok()
                            .and_then(|v| v.parse().ok());
                    }
                }
            }
            Ok((s, quiche::h3::Event::Data)) if s == sid => {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = h3.recv_body(conn, s, &mut chunk) {
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                }
            }
            Ok((s, quiche::h3::Event::Finished)) if s == sid => *finished = true,
            Ok(_) => {}
            Err(quiche::h3::Error::Done) | Err(_) => break,
        }
    }
}

/// One gRPC-over-H3 session on a fresh QUIC connection: `until_cancel` issues unary RPCs
/// until cancelled (sustained pressure on the per-RPC response-trailer egress), else exactly
/// `rpcs` then closes (the stream/fd reclaim probe).
///
/// Verifies status 200, a byte-identical echo, AND `grpc-status: 0` — the trailer the S29 fix
/// preserves — so an F-S29-1 regression under load surfaces as `err()`, not a silent pass.
#[allow(clippy::too_many_arguments)]
async fn grpc_h3_unary_session(
    target: SocketAddr,
    sni: &str,
    ca_path: &std::path::Path,
    seed: u64,
    rpcs: usize,
    until_cancel: bool,
    stats: &LoadStats,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(("127.0.0.1", 0)).await?;
    let local = socket.local_addr()?;
    let mut cfg = quic_client_config(ca_path)?;
    let scid = random_cid();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(Some(sni), &scid_ref, local, target, &mut cfg)
        .map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;
    let mut out = vec![0u8; MAX_UDP];
    let mut inb = vec![0u8; MAX_UDP];

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    flush(&mut conn, &socket, &mut out).await?;
    while !conn.is_established() {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("handshake timeout");
        }
        recv_one(
            &mut conn,
            &socket,
            local,
            &mut inb,
            Duration::from_millis(50),
        )
        .await;
        flush(&mut conn, &socket, &mut out).await?;
        if conn.is_closed() {
            anyhow::bail!("closed during handshake");
        }
    }
    let h3cfg = quiche::h3::Config::new().map_err(|e| anyhow::anyhow!("h3::Config: {e:?}"))?;
    let mut h3 = quiche::h3::Connection::with_transport(&mut conn, &h3cfg)
        .map_err(|e| anyhow::anyhow!("h3 with_transport: {e:?}"))?;
    flush(&mut conn, &socket, &mut out).await?;

    let mut i = 0usize;
    while (until_cancel && !cancel.is_cancelled()) || (!until_cancel && i < rpcs) {
        if conn.is_closed() {
            anyhow::bail!("conn closed mid-session");
        }
        let len = 2048 + (((seed as usize).wrapping_add(i)) % 2048);
        let payload: Vec<u8> = (0..len)
            .map(|k| ((k as u64).wrapping_add(seed).wrapping_add(i as u64) % 251) as u8)
            .collect();
        let framed = frame_grpc(&payload);

        let headers = [
            quiche::h3::Header::new(b":method", b"POST"),
            quiche::h3::Header::new(b":scheme", b"https"),
            quiche::h3::Header::new(b":authority", sni.as_bytes()),
            quiche::h3::Header::new(b":path", b"/echo.Echo/Unary"),
            quiche::h3::Header::new(b"content-type", b"application/grpc"),
            quiche::h3::Header::new(b"te", b"trailers"),
        ];
        let sid = loop {
            match h3.send_request(&mut conn, &headers, false) {
                Ok(s) => break s,
                Err(quiche::h3::Error::StreamBlocked) | Err(quiche::h3::Error::Done) => {
                    flush(&mut conn, &socket, &mut out).await?;
                    recv_one(
                        &mut conn,
                        &socket,
                        local,
                        &mut inb,
                        Duration::from_millis(10),
                    )
                    .await;
                }
                Err(e) => anyhow::bail!("send_request: {e:?}"),
            }
            if conn.is_closed() {
                anyhow::bail!("closed before request");
            }
        };
        // Send the framed message + FIN (track the offset; send_body may accept partially).
        let send_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut off = 0usize;
        while off < framed.len() {
            match h3.send_body(&mut conn, sid, framed.get(off..).unwrap_or(&[]), true) {
                Ok(n) => off += n,
                Err(quiche::h3::Error::Done) => {
                    flush(&mut conn, &socket, &mut out).await?;
                    recv_one(
                        &mut conn,
                        &socket,
                        local,
                        &mut inb,
                        Duration::from_millis(5),
                    )
                    .await;
                }
                Err(e) => anyhow::bail!("send_body: {e:?}"),
            }
            if tokio::time::Instant::now() > send_deadline {
                anyhow::bail!("send timeout");
            }
        }
        flush(&mut conn, &socket, &mut out).await?;

        let mut status = None;
        let mut grpc_status = None;
        let mut body = Vec::new();
        let mut finished = false;
        let rsp_deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while !finished {
            if tokio::time::Instant::now() > rsp_deadline {
                anyhow::bail!("response timeout (sid {sid})");
            }
            recv_one(
                &mut conn,
                &socket,
                local,
                &mut inb,
                Duration::from_millis(20),
            )
            .await;
            grpc_h3_drain(
                &mut h3,
                &mut conn,
                sid,
                &mut status,
                &mut grpc_status,
                &mut body,
                &mut finished,
            );
            flush(&mut conn, &socket, &mut out).await?;
            if conn.is_closed() {
                anyhow::bail!("closed mid-response");
            }
        }
        if status != Some(200) {
            anyhow::bail!("status {status:?}");
        }
        if grpc_status != Some(0) {
            anyhow::bail!("grpc-status {grpc_status:?} (F-S29-1 trailer drop?)");
        }
        if body != framed {
            anyhow::bail!("echo mismatch ({} vs {} bytes)", body.len(), framed.len());
        }
        stats.ok();
        i += 1;
    }
    let _ = conn.close(true, 0x100, b"done");
    let _ = flush(&mut conn, &socket, &mut out).await;
    Ok(())
}

/// Sustained gRPC-over-H3 load: `concurrency` workers, each holding a QUIC connection and issuing unary RPCs back-to-back until cancelled.
pub async fn run_grpc_h3_load(
    target: SocketAddr,
    sni: String,
    ca: PathBuf,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let (sni, ca, stats, cancel) =
            (sni.clone(), ca.clone(), Arc::clone(&stats), cancel.clone());
        workers.push(tokio::spawn(async move {
            let mut seed = (w as u64).wrapping_mul(17).wrapping_add(3);
            while !cancel.is_cancelled() {
                seed = seed.wrapping_add(101);
                if let Err(e) = grpc_h3_unary_session(
                    target,
                    &sni,
                    &ca,
                    seed,
                    usize::MAX,
                    true,
                    &stats,
                    &cancel,
                )
                .await
                {
                    if !cancel.is_cancelled() {
                        stats.err();
                        eprintln!("[grpc_h3_load err] {e}");
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// gRPC-over-H3 churn: open a connection, do `rpcs_per_cycle` unary RPCs, close, repeat — the
/// per-RPC stream + connection/fd reclaim probe (the leak class F-S29-1 also touched).
pub async fn run_grpc_h3_churn(
    target: SocketAddr,
    sni: String,
    ca: PathBuf,
    concurrency: usize,
    rpcs_per_cycle: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let (sni, ca, stats, cancel) =
            (sni.clone(), ca.clone(), Arc::clone(&stats), cancel.clone());
        workers.push(tokio::spawn(async move {
            let mut seed = (w as u64).wrapping_mul(29).wrapping_add(7);
            while !cancel.is_cancelled() {
                seed = seed.wrapping_add(53);
                if let Err(e) = grpc_h3_unary_session(
                    target,
                    &sni,
                    &ca,
                    seed,
                    rpcs_per_cycle,
                    false,
                    &stats,
                    &cancel,
                )
                .await
                {
                    if !cancel.is_cancelled() {
                        stats.err();
                        eprintln!("[grpc_h3_churn err] {e}");
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_sizes_cycle() {
        use hyper::body::Body as _;
        assert_eq!(body_for(0).size_hint().exact(), Some(0));
        assert_eq!(body_for(2).size_hint().exact(), Some(4096));
    }

    #[test]
    fn load_stats_count() {
        let s = LoadStats::new();
        s.ok();
        s.ok();
        s.err();
        assert_eq!(s.ok_count(), 2);
        assert_eq!(s.err_count(), 1);
    }

    /// F-S20-1: drive the REAL `quic_session` client against a QUIC echo backend. 4 streams ×
    /// 4096 B exceeds the ~13.5 KB initial cwnd, so the 4th stream's first `stream_send` is a
    /// PARTIAL write — the exact condition the pre-fix single-shot client mishandled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn quic_session_full_send_multistream_echoes() {
        let dir = std::env::temp_dir().join(format!(
            "lb-soak-loadgen-qs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let certs = crate::config_gen::generate_certs(&dir, "echo-backend.test").unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let backend = crate::backends::spawn_quic_echo_backend(
            certs.cert.clone(),
            certs.key.clone(),
            Arc::clone(&stop),
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        let r = quic_session(backend, "echo-backend.test", &certs.ca, 4, 4096, 2).await;

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&dir);
        r.expect("4 concurrent streams must echo end-to-end via the partial-write re-send loop");
    }

    /// sc8_ws_h1 self-test: non-vacuous — it byte-verifies a real RFC 6455 echo round-trip,
    /// so a broken handshake or a wrong echo fails.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_h1_open_close_cycle_echoes() {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let backend = crate::backends::spawn_ws_h1_backend(Arc::clone(&stop))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let served = ws_h1_open_close_cycle(backend, 1, 3).await;
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let n = served.expect("a clean WS open→echo→close cycle must round-trip");
        assert_eq!(n, 3, "all 3 echo frames must be verified");
    }

    /// The sustained echo loop must record `ok`s and exit cleanly when the cancel fires (no hang).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_h1_echo_loop_counts_and_cancels() {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let backend = crate::backends::spawn_ws_h1_backend(Arc::clone(&stop))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let cancel = CancellationToken::new();
        let stats = LoadStats::new();
        let c2 = cancel.clone();
        let s2 = Arc::clone(&stats);
        let h = tokio::spawn(async move { ws_h1_echo_loop(backend, 5, &c2, &s2).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("echo loop must exit promptly after cancel")
            .expect("join");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        r.expect("the echo loop must end cleanly on cancel, not error");
        assert!(
            stats.ok_count() >= 1,
            "the sustained echo loop must verify at least one round-trip"
        );
    }

    // The per-request VERDICT logic in `h3_one_request` must be proven non-vacuous on its own:
    // it must accept a real 400+body round-trip AND REJECT a 200 on the bad-authority class.

    /// Build a loopback client/server `quiche::h3` pair, handshaken and ready.
    async fn h3_pair_with_status(
        status: u16,
    ) -> (
        quiche::Connection,
        quiche::h3::Connection,
        Arc<UdpSocket>,
        SocketAddr,
        tokio::task::JoinHandle<()>,
        CancellationToken,
        std::path::PathBuf,
    ) {
        h3_pair_build(Some(status)).await
    }

    /// Build a loopback client/server `quiche::h3` pair. `status = Some(s)` ⇒ the server
    /// answers the first request with `:status s` + body; `None` ⇒ it drains the request but
    /// NEVER responds (the analogue of the production no-backend silent drop).
    async fn h3_pair_build(
        status: Option<u16>,
    ) -> (
        quiche::Connection,
        quiche::h3::Connection,
        Arc<UdpSocket>,
        SocketAddr,
        tokio::task::JoinHandle<()>,
        CancellationToken,
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "lb-soak-h3client-{}-{}-{}",
            std::process::id(),
            status.map_or(0, u32::from),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let certs = crate::config_gen::generate_certs(&dir, "h3.test").unwrap();

        let server_sock = Arc::new(UdpSocket::bind(("127.0.0.1", 0)).await.unwrap());
        let server_local = server_sock.local_addr().unwrap();
        let client_sock = Arc::new(UdpSocket::bind(("127.0.0.1", 0)).await.unwrap());
        let client_local = client_sock.local_addr().unwrap();

        let mut scfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        scfg.set_application_protos(&[b"h3"]).unwrap();
        scfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
            .unwrap();
        scfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
            .unwrap();
        scfg.set_max_idle_timeout(8_000);
        scfg.set_max_recv_udp_payload_size(1_350);
        scfg.set_max_send_udp_payload_size(1_350);
        scfg.set_initial_max_data(1024 * 1024);
        scfg.set_initial_max_stream_data_bidi_local(256 * 1024);
        scfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
        scfg.set_initial_max_stream_data_uni(256 * 1024);
        scfg.set_initial_max_streams_bidi(16);
        scfg.set_initial_max_streams_uni(16);
        scfg.set_disable_active_migration(true);

        let scid = random_cid();
        let scid_ref = quiche::ConnectionId::from_ref(&scid);
        let mut server_conn =
            quiche::accept(&scid_ref, None, server_local, client_local, &mut scfg).unwrap();

        let mut ccfg = quic_client_config(&certs.ca).unwrap();
        let c_scid = random_cid();
        let c_scid_ref = quiche::ConnectionId::from_ref(&c_scid);
        let mut client_conn = quiche::connect(
            Some("h3.test"),
            &c_scid_ref,
            client_local,
            server_local,
            &mut ccfg,
        )
        .unwrap();

        let mut out = vec![0u8; MAX_UDP];
        let mut inb = vec![0u8; MAX_UDP];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !(server_conn.is_established() && client_conn.is_established()) {
            assert!(tokio::time::Instant::now() < deadline, "handshake stalled");
            flush(&mut client_conn, &client_sock, &mut out)
                .await
                .unwrap();
            flush(&mut server_conn, &server_sock, &mut out)
                .await
                .unwrap();
            recv_one(
                &mut server_conn,
                &server_sock,
                server_local,
                &mut inb,
                Duration::from_millis(20),
            )
            .await;
            recv_one(
                &mut client_conn,
                &client_sock,
                client_local,
                &mut inb,
                Duration::from_millis(20),
            )
            .await;
        }

        let ch3cfg = quiche::h3::Config::new().unwrap();
        let client_h3 = quiche::h3::Connection::with_transport(&mut client_conn, &ch3cfg).unwrap();
        flush(&mut client_conn, &client_sock, &mut out)
            .await
            .unwrap();

        let cancel = CancellationToken::new();
        let srv_cancel = cancel.clone();
        let srv_sock = Arc::clone(&server_sock);
        let server = tokio::spawn(async move {
            let sh3cfg = quiche::h3::Config::new().unwrap();
            let mut sh3: Option<quiche::h3::Connection> = None;
            let mut out = vec![0u8; MAX_UDP];
            let mut inb = vec![0u8; MAX_UDP];
            while !srv_cancel.is_cancelled() && !server_conn.is_closed() {
                if sh3.is_none() && server_conn.is_established() {
                    if let Ok(h) = quiche::h3::Connection::with_transport(&mut server_conn, &sh3cfg)
                    {
                        sh3 = Some(h);
                    }
                }
                let _ = flush(&mut server_conn, &srv_sock, &mut out).await;
                recv_one(
                    &mut server_conn,
                    &srv_sock,
                    server_local,
                    &mut inb,
                    Duration::from_millis(20),
                )
                .await;
                if let Some(h) = sh3.as_mut() {
                    while let Ok((sid, ev)) = h.poll(&mut server_conn) {
                        match ev {
                            quiche::h3::Event::Headers { .. } | quiche::h3::Event::Data => {
                                let mut b = [0u8; 4096];
                                while let Ok(n) = h.recv_body(&mut server_conn, sid, &mut b) {
                                    if n == 0 {
                                        break;
                                    }
                                }
                                // `None` ⇒ the no-backend silent-drop analogue: drain, never
                                // respond, never reset.
                                if let Some(s) = status {
                                    let st = s.to_string();
                                    let resp = [quiche::h3::Header::new(b":status", st.as_bytes())];
                                    if h.send_response(&mut server_conn, sid, &resp, false).is_ok()
                                    {
                                        let _ = h.send_body(
                                            &mut server_conn,
                                            sid,
                                            b"bad request",
                                            true,
                                        );
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let _ = flush(&mut server_conn, &srv_sock, &mut out).await;
            }
        });

        (
            client_conn,
            client_h3,
            client_sock,
            client_local,
            server,
            cancel,
            dir,
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn h3_one_request_accepts_real_400_roundtrip() {
        let (mut conn, mut h3, sock, local, server, cancel, dir) = h3_pair_with_status(400).await;
        let mut out = vec![0u8; MAX_UDP];
        let mut inb = vec![0u8; MAX_UDP];
        let r = h3_one_request(
            &mut conn, &mut h3, &sock, local, &mut out, &mut inb, true, 0,
        )
        .await;
        cancel.cancel();
        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
        let outcome =
            r.expect("client must accept a real 400 + body round-trip on the bad-authority class");
        assert!(
            matches!(outcome, H3Outcome::Verified400),
            "the bad-authority round-trip must report Verified400"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn h3_one_request_rejects_wrong_status_on_bad_authority() {
        // Load-bearing negative control: a (wrong) 200 on the bad-authority class MUST be an
        // Err — proves the status check is not vacuous.
        let (mut conn, mut h3, sock, local, server, cancel, dir) = h3_pair_with_status(200).await;
        let mut out = vec![0u8; MAX_UDP];
        let mut inb = vec![0u8; MAX_UDP];
        let r = h3_one_request(
            &mut conn, &mut h3, &sock, local, &mut out, &mut inb, true, 0,
        )
        .await;
        cancel.cancel();
        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            r.is_err(),
            "a 200 on the bad-authority class must FAIL the verdict (got Ok — vacuous check)"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn h3_one_request_valid_authority_silent_drop_is_bounded() {
        // The valid-authority class mirrors the production no-backend drop: the client must
        // observe NO :status and report BoundedDrop — NOT hang, NOT mis-count it as a failure.
        let (mut conn, mut h3, sock, local, server, cancel, dir) = h3_pair_no_response().await;
        let mut out = vec![0u8; MAX_UDP];
        let mut inb = vec![0u8; MAX_UDP];
        let started = std::time::Instant::now();
        let r = h3_one_request(
            &mut conn, &mut h3, &sock, local, &mut out, &mut inb, false, 0,
        )
        .await;
        let elapsed = started.elapsed();
        cancel.cancel();
        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
        let outcome = r.expect("a silent drop on the valid-authority class must be a BoundedDrop");
        assert!(
            matches!(outcome, H3Outcome::BoundedDrop),
            "valid-authority silent drop must report BoundedDrop"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "the drop path must not hang — bounded by the short drop-window, took {elapsed:?}"
        );
    }

    /// A client/server H3 pair whose server accepts + handshakes but NEVER answers — the
    /// in-process analogue of the production no-backend silent drop.
    async fn h3_pair_no_response() -> (
        quiche::Connection,
        quiche::h3::Connection,
        Arc<UdpSocket>,
        SocketAddr,
        tokio::task::JoinHandle<()>,
        CancellationToken,
        std::path::PathBuf,
    ) {
        h3_pair_build(None).await
    }
}
