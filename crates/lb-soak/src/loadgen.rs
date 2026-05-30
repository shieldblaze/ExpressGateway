//! Sustained load drivers, one per datapath. Each `run_*_load` spawns
//! `concurrency` workers that loop a unit of work (a request / a connection)
//! until the shared [`CancellationToken`] fires, recording ok/err counts in a
//! shared [`LoadStats`]. The goal is sustained, churning concurrency — NOT
//! throughput — so the workers favour connection turnover over pipelining.
//!
//! * H1 — hyper http1 client with keep-alive reuse + periodic close (churn).
//! * H2 — rustls (ALPN h2) + hyper http2 client, batches of streams per conn.
//! * QUIC — quiche client (Mode A passthrough OR Mode B terminate): per
//!   connection it opens streams, byte-verifies the echo, optionally floods
//!   datagrams, then closes — exercising connection + stream + datagram churn.

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

/// Shared success/error tally for a load driver (liveness + sanity, not a
/// throughput SLO).
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

/// H1 keep-alive + churn load. Each worker opens a connection, issues a short
/// burst of keep-alive requests (mixed GET/POST bodies), then closes — so the
/// gateway sees both sustained requests and connection turnover.
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

/// One connection: up to `burst` keep-alive requests. Returns the count served.
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

/// H2-over-TLS load (front is `h1s`, ALPN selects h2). Each worker establishes
/// a TLS+H2 connection, issues a batch of (concurrent) request streams, then
/// closes.
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
            // Cannot build the TLS config — record one error and bail rather
            // than spin.
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
                    Err(_) => stats.err(),
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// Build a rustls TLS connector that trusts the CA at `ca_path` and advertises
/// ALPN `h2`. Shared with the chaos injectors (rapid-reset / stream-flood).
pub fn h2_tls_connector(ca_path: &std::path::Path) -> anyhow::Result<tokio_rustls::TlsConnector> {
    let ca_pem = std::fs::read(ca_path)?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &ca_pem[..]) {
        roots.add(cert?)?;
    }
    let mut cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
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
    // Fire a batch of streams; await each (bounded) so the connection sees real
    // concurrent stream churn.
    let mut futs = Vec::new();
    for i in 0..batch {
        let body = body_for(seed.wrapping_add(i as u64));
        let method = if i % 2 == 0 { "GET" } else { "POST" };
        let req = Request::builder()
            .method(method)
            .uri("/")
            .header("host", sni)
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

/// QUIC load for Mode A (passthrough; `target` is the gateway passthrough bind,
/// `ca` trusts the BACKEND, `sni` is the backend's) OR Mode B (terminate;
/// `target` is the gateway QUIC listener, `ca` trusts the GATEWAY, `sni` is the
/// front cert's). Each worker repeatedly opens a connection, runs
/// `streams_per_conn` echo-verified bidi streams, optionally floods datagrams,
/// then closes.
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
                    Err(_) => stats.err(),
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

/// One full client connection lifecycle: handshake → N echo-verified bidi
/// streams → optional datagram flood → close. Returns Err on handshake failure
/// or echo mismatch (a real relay defect would surface here).
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

    // Handshake.
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
    // Open streams_per_conn bidi streams; send payload+FIN on each.
    let mut expecting: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for s in 0..streams_per_conn {
        let sid = (s as u64) * 4; // client-initiated bidi stream ids: 0,4,8,…
        if conn.stream_send(sid, &payload, true).is_ok() {
            expecting.insert(sid, 0);
        }
    }
    // Datagram flood (drop-newest is tested on the gateway's bounded queue).
    for _ in 0..datagrams_per_conn {
        let _ = conn.dgram_send(&payload[..payload_len.min(1024)]);
    }
    flush(&mut conn, &socket, &mut out).await?;

    // Pump until every stream's echo is fully received or the deadline hits.
    let relay_deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    while !expecting.is_empty() {
        if tokio::time::Instant::now() > relay_deadline || conn.is_closed() {
            anyhow::bail!("relay timeout / closed (streams left: {})", expecting.len());
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
        // Drain echoed datagrams (don't assert — drop-newest may shed some).
        while conn.dgram_recv(&mut rd).is_ok() {}
        flush(&mut conn, &socket, &mut out).await?;
    }

    // Graceful close.
    let _ = conn.close(true, 0x0, b"done");
    let _ = flush(&mut conn, &socket, &mut out).await;
    Ok(())
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
}
