//! Chaos injectors — clients that deliberately misbehave to stress the
//! gateway's admission / timeout / reset-accounting paths. Each `run_*` loops
//! until the shared [`CancellationToken`] fires.
//!
//! The soak's question for every injector is the same R8 one: does the gateway
//! stay BOUNDED (fd / connection-table / stream-table / RSS flat) while the
//! abuse is sustained? The injectors create the pressure; the sampler watches
//! the bound.
//!
//! * conn-flood — rapid TCP connect/close churn (accept path, per-IP cap, fd).
//! * slowloris — many connections dribbling a partial header forever (header
//!   timeout must reap them; fd bounded).
//! * slow-POST — full headers then a trickled body (body timeout must reap).
//! * mid-stream disconnect — start a request, abruptly drop mid-response.
//! * oversize + teardown — over-cap request over TLS, torn down mid-reply
//!   (reproduces CF-S19-TLS-TEARDOWN-413 under load).
//! * rapid-reset — H2 open-stream-then-reset churn (CVE-2023-44487 accounting).
//! * stream-flood — H2 hold many concurrent streams (max_concurrent_streams).
//!
//! Datagram-flood lives in [`crate::loadgen`] (it is a property of the QUIC
//! session, not a separate TCP injector).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::loadgen::{LoadStats, h2_tls_connector};

/// Rapid TCP connect → (optional tiny write) → close, at `concurrency`
/// parallel loopers. Exercises the accept path, the per-IP connection cap, and
/// fd churn.
pub async fn run_conn_flood(
    target: SocketAddr,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        workers.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                match tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(target)).await
                {
                    Ok(Ok(mut s)) => {
                        // Half a request line then immediate close — pure churn.
                        let _ = s.write_all(b"GET / HT").await;
                        drop(s);
                        stats.ok();
                    }
                    _ => stats.err(),
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// Hold `n_conns` connections open, each having sent only a partial request
/// header (no terminating CRLF-CRLF), dribbling a header byte every few seconds
/// to look alive. The gateway's header-read timeout must reap them, so fd stays
/// bounded. Reaped connections are re-established.
pub async fn run_slowloris(target: SocketAddr, n_conns: usize, cancel: CancellationToken) {
    let mut workers = Vec::new();
    for w in 0..n_conns {
        let cancel = cancel.clone();
        workers.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                if let Ok(Ok(mut s)) =
                    tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(target)).await
                {
                    let _ = s.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n").await;
                    // Dribble a bogus header byte periodically; never finish.
                    let mut ticks = 0u64;
                    while !cancel.is_cancelled() && ticks < 30 {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        if s.write_all(format!("X-{w}-{ticks}: y\r\n").as_bytes())
                            .await
                            .is_err()
                        {
                            break; // reaped by the gateway timeout
                        }
                        ticks += 1;
                    }
                } else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// Send a complete header block declaring a large `Content-Length`, then
/// trickle the body one byte at a time. The body-read timeout must reap the
/// connection (bounded fd), not buffer unboundedly.
pub async fn run_slow_post(target: SocketAddr, n_conns: usize, cancel: CancellationToken) {
    let mut workers = Vec::new();
    for _ in 0..n_conns {
        let cancel = cancel.clone();
        workers.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                if let Ok(Ok(mut s)) =
                    tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(target)).await
                {
                    let head =
                        b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1000000\r\n\r\n";
                    let _ = s.write_all(head).await;
                    let mut sent = 0u64;
                    while !cancel.is_cancelled() && sent < 1_000_000 {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        if s.write_all(b"x").await.is_err() {
                            break; // reaped
                        }
                        sent += 1;
                    }
                } else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// Begin a normal request, read part of the response, then abruptly drop the
/// socket mid-response. Stresses the gateway's downstream-abort / cleanup path
/// repeatedly (no half-closed leak).
pub async fn run_mid_stream_disconnect(
    target: SocketAddr,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        workers.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                if let Ok(Ok(mut s)) =
                    tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(target)).await
                {
                    let _ = s
                        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                        .await;
                    let mut buf = [0u8; 8];
                    let _ = tokio::time::timeout(Duration::from_millis(50), s.read(&mut buf)).await;
                    drop(s); // abrupt mid-response close
                    stats.ok();
                } else {
                    stats.err();
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// Over TLS (h1s front), send an over-cap request (an oversized header block
/// that trips the gateway's header-list / 4xx path) and then tear the TLS
/// connection down mid-reply. This reproduces CF-S19-TLS-TEARDOWN-413 — the
/// TLS-teardown-vs-error-head race — under sustained load. `stats.err()` counts
/// connections where the teardown raced the head (no clean status read); a
/// non-zero-but-bounded err rate is expected, a panic/leak is the finding.
pub async fn run_oversize_teardown(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let connector = match h2_tls_connector(&ca_path) {
        Ok(c) => c,
        Err(_) => {
            stats.err();
            return;
        }
    };
    // ~80 KiB of header bytes — exceeds the default 64 KiB max_header_list_size.
    let big_value = "a".repeat(80 * 1024);
    let mut workers = Vec::new();
    for w in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let connector = connector.clone();
        let sni = sni.clone();
        let big_value = big_value.clone();
        workers.push(tokio::spawn(async move {
            // CF-S19 (S21): SHARPER teardown-vs-error-head race. The error head
            // for every `error_response` (4xx/413/502) flows through the SAME
            // buffered `Bytes` body returned to hyper (h2_proxy.rs::error_response),
            // so a cheap oversize-HEADER 4xx exercises the identical
            // response-flush-vs-teardown window the 413 would (the 413-only code
            // is the >64 MiB body buffering BEFORE the flush, unrelated to
            // teardown timing — and flooding 64 MiB bodies would saturate the
            // box, the S20 anti-pattern). We sweep the abort delay across the
            // sub-millisecond..few-ms window where the gateway is mid-flush, to
            // maximise the chance of catching any teardown race.
            let mut iter = w as u64;
            while !cancel.is_cancelled() {
                iter = iter.wrapping_add(1);
                // 0,1,2,4,8 ms cycle — 0ms = drop the instant send_request is
                // issued (head not yet read), the tightest race.
                let abort_after = match iter % 5 {
                    0 => Duration::from_millis(0),
                    1 => Duration::from_millis(1),
                    2 => Duration::from_millis(2),
                    3 => Duration::from_millis(4),
                    _ => Duration::from_millis(8),
                };
                match oversize_once(&connector, target, &sni, &big_value, abort_after).await {
                    Ok(true) => stats.ok(),   // saw a clean 4xx head
                    Ok(false) => stats.err(), // teardown raced the head (expected, bounded)
                    Err(_) => stats.err(),
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

async fn oversize_once(
    connector: &tokio_rustls::TlsConnector,
    target: SocketAddr,
    sni: &str,
    big_value: &str,
    abort_after: Duration,
) -> anyhow::Result<bool> {
    let tcp = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(target)).await??;
    let server_name = rustls_pki_types::ServerName::try_from(sni.to_string())?;
    let tls =
        tokio::time::timeout(Duration::from_secs(3), connector.connect(server_name, tcp)).await??;
    let (mut sender, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls)).await?;
    let driver = tokio::spawn(conn);
    let req = Request::builder()
        .method("GET")
        .uri(format!("https://{sni}/"))
        .header("x-oversize", big_value)
        .body(Full::new(Bytes::new()))?;
    // Race the (likely 4xx) error head against a TIGHT teardown: issue the
    // request, then abort the connection after `abort_after` (0ms = drop while
    // the head is still in flight). Returns whether we observed a clean head.
    let saw_head = tokio::select! {
        biased;
        () = tokio::time::sleep(abort_after) => false,
        r = sender.send_request(req) => r.is_ok(),
    };
    drop(sender);
    driver.abort();
    Ok(saw_head)
}

/// H2 rapid-reset churn (CVE-2023-44487 accounting): over a TLS+h2 connection,
/// open a stream and immediately abort it (RST_STREAM), as fast as possible.
/// When the gateway's reset-accounting trips and GOAWAYs, the connection
/// breaks and is re-established. The bound under test: this must not grow
/// memory / streams unboundedly.
pub async fn run_rapid_reset(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    stats: Arc<LoadStats>,
    cancel: CancellationToken,
) {
    let connector = match h2_tls_connector(&ca_path) {
        Ok(c) => c,
        Err(_) => {
            stats.err();
            return;
        }
    };
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let connector = connector.clone();
        let sni = sni.clone();
        workers.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                let Ok(tcp) =
                    tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(target)).await
                else {
                    stats.err();
                    continue;
                };
                let Ok(tcp) = tcp else {
                    stats.err();
                    continue;
                };
                let Ok(server_name) = rustls_pki_types::ServerName::try_from(sni.clone()) else {
                    stats.err();
                    continue;
                };
                let Ok(Ok(tls)) = tokio::time::timeout(
                    Duration::from_secs(3),
                    connector.connect(server_name, tcp),
                )
                .await
                else {
                    stats.err();
                    continue;
                };
                let Ok((sender, conn)) =
                    hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
                        .await
                else {
                    stats.err();
                    continue;
                };
                let driver = tokio::spawn(conn);
                // Open-then-reset as fast as the gateway tolerates.
                let mut n = 0u32;
                while !cancel.is_cancelled() && n < 500 {
                    let mut s = sender.clone();
                    let req = Request::builder()
                        .method("GET")
                        .uri(format!("https://{sni}/"))
                        .body(Full::new(Bytes::new()));
                    let Ok(req) = req else { break };
                    // Spawn so hyper emits HEADERS, then abort → RST_STREAM.
                    let h = tokio::spawn(async move {
                        let _ = s.send_request(req).await;
                    });
                    tokio::task::yield_now().await;
                    h.abort();
                    n += 1;
                    if driver.is_finished() {
                        break; // gateway GOAWAY'd / connection closed
                    }
                }
                for _ in 0..n {
                    stats.ok();
                }
                driver.abort();
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// H2 concurrent-stream flood: hold many in-flight streams open at once,
/// pressing on `max_concurrent_streams` (default 256). The cap must bound the
/// stream table; this must not leak.
pub async fn run_stream_flood(
    target: SocketAddr,
    sni: String,
    ca_path: PathBuf,
    concurrency: usize,
    cancel: CancellationToken,
) {
    let connector = match h2_tls_connector(&ca_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let cancel = cancel.clone();
        let connector = connector.clone();
        let sni = sni.clone();
        workers.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                let Ok(Ok(tcp)) =
                    tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(target)).await
                else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                };
                let Ok(server_name) = rustls_pki_types::ServerName::try_from(sni.clone()) else {
                    continue;
                };
                let Ok(Ok(tls)) = tokio::time::timeout(
                    Duration::from_secs(3),
                    connector.connect(server_name, tcp),
                )
                .await
                else {
                    continue;
                };
                let Ok((sender, conn)) =
                    hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
                        .await
                else {
                    continue;
                };
                let driver = tokio::spawn(conn);
                // Launch many streams and HOLD them (don't await) to press the
                // concurrent-stream cap, for a bounded window, then recycle.
                let mut holders = Vec::new();
                for _ in 0..300 {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let mut s = sender.clone();
                    let req = Request::builder()
                        .method("POST")
                        .uri(format!("https://{sni}/"))
                        // Large CL but we never send the body → stream stays open.
                        .header("content-length", "1000000")
                        .body(Full::new(Bytes::new()));
                    let Ok(req) = req else { break };
                    holders.push(tokio::spawn(async move {
                        let _ = s.send_request(req).await;
                    }));
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                for h in holders {
                    h.abort();
                }
                driver.abort();
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

    #[tokio::test]
    async fn conn_flood_stops_on_cancel() {
        // Point at a closed port; the flood should still loop+error and stop
        // promptly on cancel (no hang).
        let stats = LoadStats::new();
        let cancel = CancellationToken::new();
        let target: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let c2 = cancel.clone();
        let h = tokio::spawn(run_conn_flood(target, 2, Arc::clone(&stats), c2));
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(3), h)
            .await
            .expect("must stop on cancel");
    }
}
