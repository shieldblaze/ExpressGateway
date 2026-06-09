//! `bench` — closed-loop latency + throughput drivers for the S39 perf
//! characterization (R12 single-sourced). Distinct from [`crate::loadgen`],
//! which records only ok/err counts for the boundedness soak: this module times
//! **every request** (`Instant` send→full-response) into a per-worker latency
//! vector, so Phase 1 can report achieved RPS + p50/p99/p999 per protocol path.
//!
//! Model: **closed-loop**. `conns` concurrent workers, each holding ONE
//! connection and issuing request units back-to-back; the per-unit RTT is
//! recorded only inside the measurement window (a warmup prefix is discarded —
//! the cold-start outlier). Throughput = recorded-ok / measure-window-secs.
//!
//! It REUSES the proven connection patterns (the quiche transport pump, the
//! accept-any TLS connector) by copying them verbatim from `loadgen` rather than
//! mutating `loadgen` — the soak path stays byte-identical (R3). The copied
//! helpers are small (flush/recv_one/random_cid/quic_client_config) and the H2
//! TLS connector is reused directly (`loadgen::h2_tls_connector` is `pub`).
//!
//! Panic-freedom: this is non-test code, so the crate's deny(unwrap/expect/
//! panic) applies — every fallible step is `?`/`match`/`unwrap_or`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::{TcpStream, UdpSocket};

const MAX_UDP: usize = 65_535;

// ──────────────────────────── latency collector ────────────────────────────

/// Per-worker latency samples (microseconds). Dependency-free: merge → sort →
/// exact percentile (no new Cargo.lock edge).
#[derive(Default)]
pub struct Lat {
    us: Vec<u64>,
}

impl Lat {
    fn record(&mut self, d: Duration) {
        // micros fits u64 for any realistic loopback RTT.
        self.us
            .push(u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
    }
}

/// One protocol/concurrency run's merged outcome.
pub struct RunOut {
    pub lat: Lat,
    pub ok: u64,
    pub err: u64,
}

impl RunOut {
    fn empty() -> Self {
        Self {
            lat: Lat::default(),
            ok: 0,
            err: 0,
        }
    }
    fn merge(mut self, other: RunOut) -> Self {
        self.lat.us.extend(other.lat.us);
        self.ok += other.ok;
        self.err += other.err;
        self
    }
}

fn pct(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let last = sorted.len() - 1;
    let idx = ((p / 100.0) * (last as f64)).round() as usize;
    *sorted.get(idx.min(last)).unwrap_or(&0)
}

/// The headline summary for one (protocol, conns) run.
pub struct BenchSummary {
    pub protocol: String,
    pub conns: usize,
    pub payload: usize,
    pub warmup_secs: f64,
    pub measure_secs: f64,
    pub ok: u64,
    pub err: u64,
    pub rps: f64,
    pub p50_us: u64,
    pub p90_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
    pub max_us: u64,
    pub mean_us: u64,
}

impl BenchSummary {
    /// Build the summary from a completed run + the true measured wall window.
    pub fn from_run(
        protocol: &str,
        conns: usize,
        payload: usize,
        warmup_secs: f64,
        measure_secs: f64,
        mut run: RunOut,
    ) -> Self {
        run.lat.us.sort_unstable();
        let s = &run.lat.us;
        let mean = if s.is_empty() {
            0
        } else {
            (s.iter().sum::<u64>()) / (s.len() as u64)
        };
        let rps = if measure_secs > 0.0 {
            (run.ok as f64) / measure_secs
        } else {
            0.0
        };
        Self {
            protocol: protocol.to_string(),
            conns,
            payload,
            warmup_secs,
            measure_secs,
            ok: run.ok,
            err: run.err,
            rps,
            p50_us: pct(s, 50.0),
            p90_us: pct(s, 90.0),
            p99_us: pct(s, 99.0),
            p999_us: pct(s, 99.9),
            max_us: s.last().copied().unwrap_or(0),
            mean_us: mean,
        }
    }

    pub fn human(&self) -> String {
        format!(
            "{p:<12} c={c:<4} payload={pl:<6} | rps={rps:>10.0} ok={ok} err={err} | \
             p50={p50}us p90={p90}us p99={p99}us p999={p999}us max={max}us mean={mean}us \
             (measure={ms:.0}s warmup={ws:.0}s)",
            p = self.protocol,
            c = self.conns,
            pl = self.payload,
            rps = self.rps,
            ok = self.ok,
            err = self.err,
            p50 = self.p50_us,
            p90 = self.p90_us,
            p99 = self.p99_us,
            p999 = self.p999_us,
            max = self.max_us,
            mean = self.mean_us,
            ms = self.measure_secs,
            ws = self.warmup_secs,
        )
    }

    pub fn to_json(&self) -> String {
        format!(
            "{{\"protocol\":\"{p}\",\"conns\":{c},\"payload\":{pl},\"warmup_secs\":{ws},\
             \"measure_secs\":{ms},\"ok\":{ok},\"err\":{err},\"rps\":{rps:.1},\
             \"p50_us\":{p50},\"p90_us\":{p90},\"p99_us\":{p99},\"p999_us\":{p999},\
             \"max_us\":{max},\"mean_us\":{mean}}}",
            p = self.protocol,
            c = self.conns,
            pl = self.payload,
            ws = self.warmup_secs,
            ms = self.measure_secs,
            ok = self.ok,
            err = self.err,
            rps = self.rps,
            p50 = self.p50_us,
            p90 = self.p90_us,
            p99 = self.p99_us,
            p999 = self.p999_us,
            max = self.max_us,
            mean = self.mean_us,
        )
    }
}

/// Measurement window: warmup is discarded, recording happens only inside
/// `[warmup_end, measure_end)`.
#[derive(Clone, Copy)]
struct Window {
    warmup_end: Instant,
    measure_end: Instant,
}

impl Window {
    fn new(warmup: Duration, measure: Duration) -> Self {
        let now = Instant::now();
        Self {
            warmup_end: now + warmup,
            measure_end: now + warmup + measure,
        }
    }
    fn measuring(&self, t: Instant) -> bool {
        t >= self.warmup_end && t < self.measure_end
    }
    fn done(&self) -> bool {
        Instant::now() >= self.measure_end
    }
}

fn payload_bytes(len: usize, seed: u64) -> Vec<u8> {
    (0..len).map(|i| ((i as u64 + seed) % 251) as u8).collect()
}

async fn join_all(workers: Vec<tokio::task::JoinHandle<RunOut>>) -> RunOut {
    let mut acc = RunOut::empty();
    for w in workers {
        if let Ok(r) = w.await {
            acc = acc.merge(r);
        }
    }
    acc
}

// ──────────────────────────── H1 (plaintext) ───────────────────────────────

/// Closed-loop H1: each worker holds a keep-alive connection, issues GET (or
/// POST when payload>0) one at a time, times send→full-body-collected.
pub async fn bench_h1(
    target: SocketAddr,
    conns: usize,
    warmup: Duration,
    measure: Duration,
    payload: usize,
) -> RunOut {
    let win = Window::new(warmup, measure);
    let mut workers = Vec::new();
    for _ in 0..conns {
        workers.push(tokio::spawn(async move {
            let mut out = RunOut::empty();
            let mut seed = 0u64;
            while !win.done() {
                let stream =
                    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(target))
                        .await
                    {
                        Ok(Ok(s)) => s,
                        _ => {
                            if win.measuring(Instant::now()) {
                                out.err += 1;
                            }
                            continue;
                        }
                    };
                // TCP_NODELAY on the LOAD CLIENT socket: without it, Nagle holds
                // small frames and the delayed-ACK timer (~40ms) inflates the
                // tail — a measurement artifact, not the gateway (which sets
                // nodelay on every socket). See S39 report §H2 tail.
                let _ = stream.set_nodelay(true);
                let (mut sender, conn) =
                    match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await {
                        Ok(x) => x,
                        Err(_) => continue,
                    };
                let driver = tokio::spawn(conn);
                loop {
                    if win.done() {
                        break;
                    }
                    seed = seed.wrapping_add(1);
                    let (method, body) = if payload > 0 {
                        ("POST", Full::new(Bytes::from(payload_bytes(payload, seed))))
                    } else {
                        ("GET", Full::new(Bytes::new()))
                    };
                    let req = match Request::builder()
                        .method(method)
                        .uri("/")
                        .header("host", "localhost")
                        .body(body)
                    {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let start = Instant::now();
                    match tokio::time::timeout(Duration::from_secs(10), sender.send_request(req))
                        .await
                    {
                        Ok(Ok(resp)) => {
                            let _ = resp.into_body().collect().await;
                            let dt = start.elapsed();
                            if win.measuring(start) {
                                out.lat.record(dt);
                                out.ok += 1;
                            }
                        }
                        _ => {
                            if win.measuring(start) {
                                out.err += 1;
                            }
                            break; // reconnect
                        }
                    }
                }
                driver.abort();
            }
            out
        }));
    }
    join_all(workers).await
}

// ──────────────────────────── H2 (TLS, ALPN h2) ────────────────────────────

/// Closed-loop H2 over TLS: persistent connection, one request at a time.
pub async fn bench_h2(
    target: SocketAddr,
    sni: String,
    ca: PathBuf,
    conns: usize,
    warmup: Duration,
    measure: Duration,
    payload: usize,
) -> RunOut {
    let win = Window::new(warmup, measure);
    let tls = match crate::loadgen::h2_tls_connector(&ca) {
        Ok(t) => t,
        Err(_) => return RunOut::empty(),
    };
    let mut workers = Vec::new();
    for _ in 0..conns {
        let tls = tls.clone();
        let sni = sni.clone();
        workers.push(tokio::spawn(async move {
            let mut out = RunOut::empty();
            let mut seed = 0u64;
            while !win.done() {
                let server_name = match rustls_pki_types::ServerName::try_from(sni.clone()) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                let tcp =
                    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(target))
                        .await
                    {
                        Ok(Ok(s)) => {
                            // TCP_NODELAY on the client socket — H2 sends small
                            // control frames (WINDOW_UPDATE) that Nagle would
                            // hold into the ~40ms delayed-ACK tail (harness
                            // artifact). The gateway already sets nodelay.
                            let _ = s.set_nodelay(true);
                            s
                        }
                        _ => {
                            if win.measuring(Instant::now()) {
                                out.err += 1;
                            }
                            continue;
                        }
                    };
                let tls_stream = match tokio::time::timeout(
                    Duration::from_secs(5),
                    tls.connect(server_name, tcp),
                )
                .await
                {
                    Ok(Ok(s)) => s,
                    _ => continue,
                };
                let (mut sender, conn) = match hyper::client::conn::http2::handshake(
                    TokioExecutor::new(),
                    TokioIo::new(tls_stream),
                )
                .await
                {
                    Ok(x) => x,
                    Err(_) => continue,
                };
                let driver = tokio::spawn(conn);
                loop {
                    if win.done() {
                        break;
                    }
                    seed = seed.wrapping_add(1);
                    let (method, body) = if payload > 0 {
                        ("POST", Full::new(Bytes::from(payload_bytes(payload, seed))))
                    } else {
                        ("GET", Full::new(Bytes::new()))
                    };
                    let req = match Request::builder()
                        .method(method)
                        .uri(format!("https://{sni}/"))
                        .body(body)
                    {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let start = Instant::now();
                    match tokio::time::timeout(Duration::from_secs(10), sender.send_request(req))
                        .await
                    {
                        Ok(Ok(resp)) => {
                            let _ = resp.into_body().collect().await;
                            let dt = start.elapsed();
                            if win.measuring(start) {
                                out.lat.record(dt);
                                out.ok += 1;
                            }
                        }
                        _ => {
                            if win.measuring(start) {
                                out.err += 1;
                            }
                            break;
                        }
                    }
                }
                driver.abort();
            }
            out
        }));
    }
    join_all(workers).await
}

// ──────────────────── QUIC transport helpers (copied from loadgen) ──────────

fn quic_client_config(ca_path: &Path) -> anyhow::Result<quiche::Config> {
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
        _ => conn.on_timeout(),
    }
}

fn random_cid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    use ring::rand::SecureRandom;
    let mut cid = [0u8; quiche::MAX_CONN_ID_LEN];
    let _ = ring::rand::SystemRandom::new().fill(&mut cid);
    cid
}

/// Open a QUIC connection + complete the handshake. Returns the connection +
/// its bound socket + local addr (the caller drives the chosen app protocol).
async fn quic_connect(
    target: SocketAddr,
    sni: &str,
    ca: &Path,
) -> anyhow::Result<(quiche::Connection, UdpSocket, SocketAddr)> {
    let socket = UdpSocket::bind(("127.0.0.1", 0)).await?;
    let local = socket.local_addr()?;
    let mut cfg = quic_client_config(ca)?;
    let scid = random_cid();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(Some(sni), &scid_ref, local, target, &mut cfg)
        .map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;
    let mut out = vec![0u8; MAX_UDP];
    let mut inb = vec![0u8; MAX_UDP];
    let deadline = Instant::now() + Duration::from_secs(8);
    flush(&mut conn, &socket, &mut out).await?;
    while !conn.is_established() {
        if Instant::now() > deadline {
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
    Ok((conn, socket, local))
}

// ──────────────────────── H3 real-proxy (H3 front → H2 backend) ─────────────

/// One H3 request unit: send headers (+ optional body), pump the transport, and
/// return when the response Finished. Records nothing — the caller times it.
async fn h3_one_request(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    socket: &UdpSocket,
    local: SocketAddr,
    out: &mut [u8],
    inb: &mut [u8],
    authority: &str,
    body: Option<&[u8]>,
) -> anyhow::Result<()> {
    let method: &[u8] = if body.is_some() { b"POST" } else { b"GET" };
    let headers = [
        quiche::h3::Header::new(b":method", method),
        quiche::h3::Header::new(b":scheme", b"https"),
        quiche::h3::Header::new(b":path", b"/"),
        quiche::h3::Header::new(b":authority", authority.as_bytes()),
    ];
    let fin = body.is_none();
    let sid = h3
        .send_request(conn, &headers, fin)
        .map_err(|e| anyhow::anyhow!("send_request: {e:?}"))?;
    if let Some(b) = body {
        let mut off = 0usize;
        while off < b.len() {
            match h3.send_body(conn, sid, b.get(off..).unwrap_or(&[]), true) {
                Ok(n) => off += n,
                Err(quiche::h3::Error::Done) => {
                    flush(conn, socket, out).await?;
                    recv_one(conn, socket, local, inb, Duration::from_micros(300)).await;
                }
                Err(e) => anyhow::bail!("send_body: {e:?}"),
            }
        }
    }
    flush(conn, socket, out).await?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Instant::now() > deadline {
            anyhow::bail!("h3 response timeout");
        }
        match h3.poll(conn) {
            Ok((stream_id, quiche::h3::Event::Headers { .. })) => {
                let _ = stream_id;
            }
            Ok((stream_id, quiche::h3::Event::Data)) => {
                while let Ok(n) = h3.recv_body(conn, stream_id, inb) {
                    if n == 0 {
                        break;
                    }
                }
            }
            Ok((_, quiche::h3::Event::Finished)) => return Ok(()),
            Ok((_, quiche::h3::Event::Reset(code))) => anyhow::bail!("reset {code}"),
            Ok((_, quiche::h3::Event::GoAway)) => anyhow::bail!("goaway"),
            Ok((_, quiche::h3::Event::PriorityUpdate)) => {}
            Err(quiche::h3::Error::Done) => {
                flush(conn, socket, out).await?;
                recv_one(conn, socket, local, inb, Duration::from_micros(300)).await;
                flush(conn, socket, out).await?;
                if conn.is_closed() {
                    anyhow::bail!("connection closed");
                }
            }
            Err(e) => anyhow::bail!("h3 poll: {e:?}"),
        }
    }
}

/// Closed-loop H3 against an H3-terminate front that proxies to a real H2
/// backend (config_gen::quic_h3_terminate_h2). Each worker holds one QUIC+H3
/// connection and issues requests one at a time.
pub async fn bench_h3(
    target: SocketAddr,
    sni: String,
    ca: PathBuf,
    conns: usize,
    warmup: Duration,
    measure: Duration,
    payload: usize,
) -> RunOut {
    let win = Window::new(warmup, measure);
    let mut workers = Vec::new();
    for _ in 0..conns {
        let sni = sni.clone();
        let ca = ca.clone();
        workers.push(tokio::spawn(async move {
            let mut out = RunOut::empty();
            let authority = format!("{sni}:443");
            let mut obuf = vec![0u8; MAX_UDP];
            let mut ibuf = vec![0u8; MAX_UDP];
            let mut seed = 0u64;
            while !win.done() {
                let (mut conn, socket, local) = match quic_connect(target, &sni, &ca).await {
                    Ok(t) => t,
                    Err(_) => {
                        if win.measuring(Instant::now()) {
                            out.err += 1;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        continue;
                    }
                };
                let h3cfg = match quiche::h3::Config::new() {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let mut h3 = match quiche::h3::Connection::with_transport(&mut conn, &h3cfg) {
                    Ok(h) => h,
                    Err(_) => continue,
                };
                let _ = flush(&mut conn, &socket, &mut obuf).await;
                while !win.done() && !conn.is_closed() {
                    seed = seed.wrapping_add(1);
                    let body_vec;
                    let body = if payload > 0 {
                        body_vec = payload_bytes(payload, seed);
                        Some(body_vec.as_slice())
                    } else {
                        None
                    };
                    let start = Instant::now();
                    let r = h3_one_request(
                        &mut conn, &mut h3, &socket, local, &mut obuf, &mut ibuf, &authority, body,
                    )
                    .await;
                    let dt = start.elapsed();
                    match r {
                        Ok(()) => {
                            if win.measuring(start) {
                                out.lat.record(dt);
                                out.ok += 1;
                            }
                        }
                        Err(_) => {
                            if win.measuring(start) {
                                out.err += 1;
                            }
                            break; // reconnect
                        }
                    }
                }
                let _ = conn.close(true, 0x100, b"done");
                let _ = flush(&mut conn, &socket, &mut obuf).await;
            }
            out
        }));
    }
    join_all(workers).await
}

// ──────────────────────── QUIC Mode A (passthrough echo) ────────────────────

/// One Mode A echo unit: open a client-initiated bidi stream, send `payload`
/// with FIN (re-sending the cwnd-bounded remainder per the F-S20-1 contract),
/// then read the full echo back. Returns when the echo's FIN arrives.
async fn quic_echo_one(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    local: SocketAddr,
    out: &mut [u8],
    inb: &mut [u8],
    rd: &mut [u8],
    sid: u64,
    payload: &[u8],
) -> anyhow::Result<()> {
    let mut send_off = 0usize;
    let mut got = 0usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Instant::now() > deadline || conn.is_closed() {
            anyhow::bail!("echo timeout (sent {send_off}/{} got {got})", payload.len());
        }
        if send_off < payload.len() {
            match conn.stream_send(sid, payload.get(send_off..).unwrap_or(&[]), true) {
                Ok(n) => send_off += n,
                Err(quiche::Error::Done) | Err(quiche::Error::StreamLimit) => {}
                Err(e) => anyhow::bail!("stream_send: {e:?}"),
            }
        }
        flush(conn, socket, out).await?;
        recv_one(conn, socket, local, inb, Duration::from_micros(300)).await;
        let readable: Vec<u64> = conn.readable().collect();
        for s in readable {
            if s != sid {
                // Drain other streams' bytes (shouldn't happen — one at a time).
                while let Ok((_n, _f)) = conn.stream_recv(s, rd) {}
                continue;
            }
            loop {
                match conn.stream_recv(sid, rd) {
                    Ok((n, fin)) => {
                        got += n;
                        if fin {
                            if got != payload.len() {
                                anyhow::bail!("echo len {got} != {}", payload.len());
                            }
                            return Ok(());
                        }
                        if n == 0 {
                            break;
                        }
                    }
                    Err(quiche::Error::Done) => break,
                    Err(e) => anyhow::bail!("stream_recv: {e:?}"),
                }
            }
        }
        flush(conn, socket, out).await?;
    }
}

/// Closed-loop QUIC Mode A passthrough: TLS is end-to-end client↔backend (the
/// gateway never decrypts); `sni`/`ca` are the BACKEND's. Each worker holds one
/// end-to-end QUIC connection and runs echo streams one at a time.
pub async fn bench_quic_modea(
    target: SocketAddr,
    backend_sni: String,
    backend_ca: PathBuf,
    conns: usize,
    warmup: Duration,
    measure: Duration,
    payload: usize,
) -> RunOut {
    let win = Window::new(warmup, measure);
    let plen = payload.max(1);
    let mut workers = Vec::new();
    for _ in 0..conns {
        let sni = backend_sni.clone();
        let ca = backend_ca.clone();
        workers.push(tokio::spawn(async move {
            let mut out = RunOut::empty();
            let mut obuf = vec![0u8; MAX_UDP];
            let mut ibuf = vec![0u8; MAX_UDP];
            let mut rdbuf = vec![0u8; MAX_UDP];
            let body = payload_bytes(plen, 7);
            while !win.done() {
                let (mut conn, socket, local) = match quic_connect(target, &sni, &ca).await {
                    Ok(t) => t,
                    Err(_) => {
                        if win.measuring(Instant::now()) {
                            out.err += 1;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        continue;
                    }
                };
                let mut next_sid = 0u64; // client bidi: 0,4,8,…
                while !win.done() && !conn.is_closed() {
                    let sid = next_sid;
                    next_sid = next_sid.wrapping_add(4);
                    let start = Instant::now();
                    let r = quic_echo_one(
                        &mut conn, &socket, local, &mut obuf, &mut ibuf, &mut rdbuf, sid, &body,
                    )
                    .await;
                    let dt = start.elapsed();
                    match r {
                        Ok(()) => {
                            if win.measuring(start) {
                                out.lat.record(dt);
                                out.ok += 1;
                            }
                        }
                        Err(_) => {
                            if win.measuring(start) {
                                out.err += 1;
                            }
                            break;
                        }
                    }
                }
                let _ = conn.close(true, 0x0, b"done");
                let _ = flush(&mut conn, &socket, &mut obuf).await;
            }
            out
        }));
    }
    join_all(workers).await
}

// ──────────────────────────── WebSocket (H1) echo ──────────────────────────

/// Closed-loop WS-over-H1: each worker holds one upgraded tunnel and times one
/// echo round-trip (send a binary frame → read its echo) at a time.
pub async fn bench_ws_h1(
    target: SocketAddr,
    conns: usize,
    warmup: Duration,
    measure: Duration,
    payload: usize,
) -> RunOut {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let win = Window::new(warmup, measure);
    let plen = payload.max(1);
    let mut workers = Vec::new();
    for _ in 0..conns {
        workers.push(tokio::spawn(async move {
            let mut out = RunOut::empty();
            let url = format!("ws://{target}/soak");
            let mut seed = 0u64;
            while !win.done() {
                let ws = match tokio::time::timeout(
                    Duration::from_secs(3),
                    tokio_tungstenite::connect_async(url.clone()),
                )
                .await
                {
                    Ok(Ok((ws, _resp))) => ws,
                    _ => {
                        if win.measuring(Instant::now()) {
                            out.err += 1;
                        }
                        continue;
                    }
                };
                let mut ws = ws;
                while !win.done() {
                    seed = seed.wrapping_add(1);
                    let payload = payload_bytes(plen, seed);
                    let start = Instant::now();
                    if tokio::time::timeout(
                        Duration::from_secs(5),
                        ws.send(Message::Binary(payload.clone().into())),
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                    let got = tokio::time::timeout(Duration::from_secs(5), async {
                        loop {
                            match ws.next().await {
                                Some(Ok(Message::Binary(b))) => return Some(b.to_vec()),
                                Some(Ok(Message::Close(_))) | None => return None,
                                Some(Ok(_)) => continue,
                                Some(Err(_)) => return None,
                            }
                        }
                    })
                    .await;
                    let dt = start.elapsed();
                    match got {
                        Ok(Some(b)) if b == payload => {
                            if win.measuring(start) {
                                out.lat.record(dt);
                                out.ok += 1;
                            }
                        }
                        _ => {
                            if win.measuring(start) {
                                out.err += 1;
                            }
                            break;
                        }
                    }
                }
                let _ = ws.close(None).await;
            }
            out
        }));
    }
    join_all(workers).await
}
