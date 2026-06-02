//! Origin servers the gateway proxies to during a soak: H1 + H2 (hyper) and a
//! multi-connection QUIC echo server (quiche) used as the far end of both Mode
//! A (passthrough, end-to-end) and Mode B (terminate/re-originate).
//!
//! Each backend is controllable at runtime via [`BackendControl`] so the chaos
//! injectors can flip it to slow / dropping without restarting it (backend-slow
//! and backend-drop chaos). The QUIC echo server mirrors the proven echo logic
//! in `crates/lb-quic/tests/s16_b2_stream_relay_smoke.rs` but demultiplexes
//! MANY concurrent connections by DCID (the gateway dials one backend
//! connection per client connection, so a soak drives many at once).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::{TcpListener, UdpSocket};

const MAX_UDP: usize = 65_535;

/// Runtime-tunable backend behaviour, shared with the chaos injectors.
#[derive(Debug, Default)]
pub struct BackendControl {
    /// Artificial per-request delay before responding (backend-slow chaos).
    slow_ms: AtomicU64,
    /// Per-mille probability of dropping a freshly-accepted connection without
    /// responding (backend-drop chaos). 0 = never.
    drop_permille: AtomicU32,
    /// Set to stop the accept loop.
    stop: AtomicBool,
    /// Total requests served (sanity / liveness).
    served: AtomicU64,
}

impl BackendControl {
    /// A new, fully-healthy control handle.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Set the artificial response delay (ms). 0 disables.
    pub fn set_slow_ms(&self, ms: u64) {
        self.slow_ms.store(ms, Ordering::Relaxed);
    }

    /// Set the per-mille connection-drop probability. 0 disables.
    pub fn set_drop_permille(&self, permille: u32) {
        self.drop_permille
            .store(permille.min(1000), Ordering::Relaxed);
    }

    /// Signal the accept loop to stop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Total requests served so far.
    #[must_use]
    pub fn served(&self) -> u64 {
        self.served.load(Ordering::Relaxed)
    }

    fn should_drop(&self) -> bool {
        let p = self.drop_permille.load(Ordering::Relaxed);
        if p == 0 {
            return false;
        }
        // Cheap deterministic-ish sampler: hash the served counter. No RNG dep.
        let n = self.served.load(Ordering::Relaxed);
        (n.wrapping_mul(2_654_435_761) % 1000) < u64::from(p)
    }

    fn slow(&self) -> Duration {
        Duration::from_millis(self.slow_ms.load(Ordering::Relaxed))
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// The service handler shared by the H1 and H2 backends: read the request body
/// fully (so the gateway's upload leg completes), honour the slow knob, then
/// return 200 with a small fixed body.
async fn handle(
    req: Request<Incoming>,
    ctrl: Arc<BackendControl>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let _ = req.into_body().collect().await; // drain upload
    let slow = ctrl.slow();
    if !slow.is_zero() {
        tokio::time::sleep(slow).await;
    }
    ctrl.served.fetch_add(1, Ordering::Relaxed);
    Ok(Response::builder()
        .status(200)
        .header("content-length", "2")
        .body(Full::new(Bytes::from_static(b"ok")))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"ok")))))
}

/// Response body that yields the echoed request bytes (one DATA frame) then a
/// `grpc-status: 0` trailing HEADERS — the gRPC happy-path shape that drives
/// the gateway's H3 response-trailer egress (the S29 F-S29-1 path) end to end.
struct GrpcEchoBody {
    data: Option<Bytes>,
    trailer_sent: bool,
}
impl hyper::body::Body for GrpcEchoBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        if let Some(d) = self.data.take() {
            return std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(d))));
        }
        if !self.trailer_sent {
            self.trailer_sent = true;
            let mut t = hyper::HeaderMap::new();
            t.insert("grpc-status", hyper::header::HeaderValue::from_static("0"));
            t.insert("grpc-message", hyper::header::HeaderValue::from_static(""));
            return std::task::Poll::Ready(Some(Ok(hyper::body::Frame::trailers(t))));
        }
        std::task::Poll::Ready(None)
    }
}

/// gRPC service handler: echo the framed request body back verbatim (the
/// gateway is opaque — no gRPC parsing) + a `grpc-status: 0` trailer.
async fn grpc_handle(
    req: Request<Incoming>,
    ctrl: Arc<BackendControl>,
) -> Result<Response<GrpcEchoBody>, std::convert::Infallible> {
    let body = req
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    let slow = ctrl.slow();
    if !slow.is_zero() {
        tokio::time::sleep(slow).await;
    }
    ctrl.served.fetch_add(1, Ordering::Relaxed);
    Ok(Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(GrpcEchoBody {
            data: Some(body),
            trailer_sent: false,
        })
        .unwrap_or_else(|_| {
            Response::new(GrpcEchoBody {
                data: None,
                trailer_sent: true,
            })
        }))
}

/// Spawn an HTTP/2 gRPC echo origin on an ephemeral loopback port (S29
/// sc9_grpc_h3). Echoes the framed request body + a `grpc-status: 0` trailing
/// HEADERS — the far end of the gateway's opaque gRPC-over-H3 relay whose
/// per-RPC stream/fd/memory bound the soak proves.
pub async fn spawn_grpc_h2_backend(ctrl: Arc<BackendControl>) -> anyhow::Result<SocketAddr> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            if ctrl.stopped() {
                return;
            }
            let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
            let Ok(Ok((stream, _))) = accept else {
                continue;
            };
            if ctrl.should_drop() {
                drop(stream);
                continue;
            }
            let ctrl2 = Arc::clone(&ctrl);
            tokio::spawn(async move {
                let svc = service_fn(move |req| grpc_handle(req, Arc::clone(&ctrl2)));
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    Ok(addr)
}

/// Spawn an HTTP/1.1 origin on an ephemeral loopback port. Returns its address.
pub async fn spawn_h1_backend(ctrl: Arc<BackendControl>) -> anyhow::Result<SocketAddr> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            if ctrl.stopped() {
                return;
            }
            let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
            let Ok(Ok((stream, _))) = accept else {
                continue;
            };
            if ctrl.should_drop() {
                drop(stream); // backend-drop: accept then close, no response
                continue;
            }
            let ctrl2 = Arc::clone(&ctrl);
            tokio::spawn(async move {
                let svc = service_fn(move |req| handle(req, Arc::clone(&ctrl2)));
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    Ok(addr)
}

/// Spawn an HTTP/2 (prior-knowledge, cleartext h2c) origin — the shape the
/// gateway's `Http2Pool` dials. Returns its address.
pub async fn spawn_h2_backend(ctrl: Arc<BackendControl>) -> anyhow::Result<SocketAddr> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            if ctrl.stopped() {
                return;
            }
            let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
            let Ok(Ok((stream, _))) = accept else {
                continue;
            };
            if ctrl.should_drop() {
                drop(stream);
                continue;
            }
            let ctrl2 = Arc::clone(&ctrl);
            tokio::spawn(async move {
                let svc = service_fn(move |req| handle(req, Arc::clone(&ctrl2)));
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    Ok(addr)
}

/// Spawn an HTTP/1.1 WebSocket echo origin (S27 sc8_ws_h1). Each accepted TCP
/// connection completes the RFC 6455 handshake via `tokio_tungstenite::
/// accept_async`, then echoes every inbound Text/Binary frame back verbatim;
/// Ping/Pong are handled by tungstenite's auto-state-machine and a Close ends
/// the per-connection task cleanly. This is the far end of the gateway's WS
/// relay (`WsProxy::proxy_frames`) — the long-lived bidirectional opaque tunnel
/// whose connection/fd/memory bound the soak proves.
///
/// Mirrors `tests/ws_proxy_e2e.rs::spawn_echo_backend`. `stop` ends the accept
/// loop (the soak signals it on teardown so the backend never outlives the run).
pub async fn spawn_ws_h1_backend(stop: Arc<AtomicBool>) -> anyhow::Result<SocketAddr> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
            let Ok(Ok((sock, _))) = accept else {
                continue;
            };
            tokio::spawn(async move {
                use futures_util::{SinkExt, StreamExt};
                use tokio_tungstenite::tungstenite::Message;
                let ws = match tokio_tungstenite::accept_async(sock).await {
                    Ok(w) => w,
                    Err(_) => return,
                };
                let (mut tx, mut rx) = ws.split();
                while let Some(Ok(msg)) = rx.next().await {
                    // The echo lives inside the match arm (not a guard) for
                    // the rust-1.95.0 clippy::collapsible_match reason
                    // documented in tests/ws_proxy_e2e.rs.
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
    Ok(addr)
}

/// Per-stream echo bookkeeping: (queued bytes still to echo, peer-FIN-seen,
/// FIN-sent).
type EchoState = (Vec<u8>, bool, bool);

/// Spawn a multi-connection QUIC echo backend. It accepts any number of
/// concurrent QUIC connections (demuxed by DCID), echoes every received bidi
/// stream's bytes back on the same stream id (FIN-aware), and echoes back any
/// received DATAGRAM. Advertises ALPN `h3` (matching the gateway listener) and
/// serves `cert`/`key`. `stop` ends the loop.
///
/// This is the far end for BOTH Mode A (the client speaks QUIC to it
/// end-to-end through the passthrough) and Mode B (the gateway dials it). It
/// does NOT speak real H3 — the raw proxy relays opaque stream bytes, so a byte
/// echo is sufficient and exactly mirrors the s16 stream-relay self-check.
pub fn spawn_quic_echo_backend(
    cert_path: std::path::PathBuf,
    key_path: std::path::PathBuf,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<SocketAddr> {
    let std_sock = std::net::UdpSocket::bind(("127.0.0.1", 0))?;
    std_sock.set_nonblocking(true)?;
    let addr = std_sock.local_addr()?;

    let mut config = quic_echo_server_config(&cert_path, &key_path)?;

    tokio::spawn(async move {
        let socket = match UdpSocket::from_std(std_sock) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut rd = vec![0u8; MAX_UDP];
        // Connections keyed by the SCID we assigned (the client's subsequent
        // DCID). Each carries its per-stream echo state.
        let mut conns: HashMap<Vec<u8>, (quiche::Connection, HashMap<u64, EchoState>)> =
            HashMap::new();

        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            // Service every live connection: read→echo→datagram→flush.
            let keys: Vec<Vec<u8>> = conns.keys().cloned().collect();
            for k in keys {
                if let Some((c, echo)) = conns.get_mut(&k) {
                    service_conn(c, echo, &socket, &mut rd, &mut out_buf).await;
                    if c.is_closed() {
                        conns.remove(&k);
                    }
                }
            }

            // Smallest timeout across conns (or a short poll if none).
            let timeout = conns
                .values()
                .filter_map(|(c, _)| c.timeout())
                .min()
                .unwrap_or(Duration::from_millis(5));

            match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    // Parse the header in a scope so its borrow of in_buf is
                    // released before we recv() into the same buffer.
                    let parsed =
                        match quiche::Header::from_slice(&mut in_buf[..n], quiche::MAX_CONN_ID_LEN)
                        {
                            Ok(h) => Some((h.dcid.to_vec(), h.ty == quiche::Type::Initial)),
                            Err(_) => None,
                        };
                    let Some((dcid, is_initial)) = parsed else {
                        continue;
                    };
                    if let Some((c, _)) = conns.get_mut(&dcid) {
                        let info = quiche::RecvInfo { from, to: addr };
                        let _ = c.recv(&mut in_buf[..n], info);
                    } else if is_initial {
                        // New connection: assign our own SCID and accept.
                        let scid = random_cid();
                        let scid_ref = quiche::ConnectionId::from_ref(&scid);
                        match quiche::accept(&scid_ref, None, addr, from, &mut config) {
                            Ok(mut c) => {
                                let info = quiche::RecvInfo { from, to: addr };
                                let _ = c.recv(&mut in_buf[..n], info);
                                conns.insert(scid.to_vec(), (c, HashMap::new()));
                            }
                            Err(_) => continue,
                        }
                    }
                }
                Ok(Err(_)) | Err(_) => {
                    for (c, _) in conns.values_mut() {
                        c.on_timeout();
                    }
                }
            }
        }
    });

    Ok(addr)
}

/// Read readable streams into the echo queues, drain them back onto the same
/// stream, echo datagrams, then flush outbound. Mirrors the s16 echo logic.
async fn service_conn(
    c: &mut quiche::Connection,
    echo: &mut HashMap<u64, EchoState>,
    socket: &UdpSocket,
    rd: &mut [u8],
    out: &mut [u8],
) {
    // 1) Stream reads.
    let readable: Vec<u64> = c.readable().collect();
    for sid in readable {
        while let Ok((n, fin)) = c.stream_recv(sid, rd) {
            let e = echo.entry(sid).or_insert((Vec::new(), false, false));
            e.0.extend_from_slice(rd.get(..n).unwrap_or(&[]));
            if fin {
                e.1 = true;
            }
            if fin || n == 0 {
                break;
            }
        }
    }
    // 2) Drain echo queues.
    let sids: Vec<u64> = echo.keys().copied().collect();
    for sid in sids {
        if let Some(e) = echo.get_mut(&sid) {
            let mut acc = 0usize;
            while acc < e.0.len() {
                let chunk = e.0.get(acc..).unwrap_or(&[]);
                match c.stream_send(sid, chunk, false) {
                    Ok(0) | Err(quiche::Error::Done) => break,
                    Ok(n) => {
                        acc += n;
                        if n < chunk.len() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            if acc > 0 {
                e.0.drain(..acc.min(e.0.len()));
            }
            if e.1 && e.0.is_empty() && !e.2 && c.stream_send(sid, &[], true).is_ok() {
                e.2 = true;
            }
        }
    }
    // 3) Echo datagrams.
    while let Ok(len) = c.dgram_recv(rd) {
        let _ = c.dgram_send(rd.get(..len).unwrap_or(&[]));
    }
    // 4) Flush outbound.
    while let Ok((n, info)) = c.send(out) {
        let _ = socket.send_to(out.get(..n).unwrap_or(&[]), info.to).await;
    }
}

fn random_cid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    use ring::rand::SecureRandom;
    let mut cid = [0u8; quiche::MAX_CONN_ID_LEN];
    let _ = ring::rand::SystemRandom::new().fill(&mut cid);
    cid
}

fn quic_echo_server_config(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> anyhow::Result<quiche::Config> {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)
        .map_err(|e| anyhow::anyhow!("quiche config: {e:?}"))?;
    cfg.set_application_protos(&[b"h3", b"h3-29"])
        .map_err(|e| anyhow::anyhow!("alpn: {e:?}"))?;
    let cert = cert_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("cert path"))?;
    let key = key_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("key path"))?;
    cfg.load_cert_chain_from_pem_file(cert)
        .map_err(|e| anyhow::anyhow!("load cert: {e:?}"))?;
    cfg.load_priv_key_from_pem_file(key)
        .map_err(|e| anyhow::anyhow!("load key: {e:?}"))?;
    cfg.set_max_idle_timeout(30_000);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_sampler_respects_zero_and_full() {
        let c = BackendControl::default();
        assert!(!c.should_drop(), "0 permille never drops");
        c.set_drop_permille(1000);
        c.served.store(1, Ordering::Relaxed);
        assert!(c.should_drop(), "1000 permille always drops");
    }

    #[test]
    fn slow_and_stop_knobs() {
        let c = BackendControl::default();
        assert!(c.slow().is_zero());
        c.set_slow_ms(50);
        assert_eq!(c.slow(), Duration::from_millis(50));
        assert!(!c.stopped());
        c.stop();
        assert!(c.stopped());
    }

    #[tokio::test]
    async fn h1_backend_serves_200() {
        let ctrl = BackendControl::new();
        let addr = spawn_h1_backend(Arc::clone(&ctrl)).await.unwrap();
        // Minimal raw H1 client.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 256];
        let n = s.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        ctrl.stop();
    }
}
