//! Chaos injectors — clients that deliberately misbehave to stress the gateway's admission, timeout
//! and reset-accounting paths. The question for every injector is the same R8 one: does the gateway
//! stay BOUNDED under it?

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

/// Over TLS (h1s front), send an over-cap request and tear the TLS connection down mid-reply —
/// reproduces CF-S19-TLS-TEARDOWN-413 (the teardown-vs-error-head race) under sustained load. A
/// bounded non-zero `stats.err()` is expected; a panic/leak is the finding.
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
            // CF-S19 (S21): a cheap oversize-HEADER 4xx flows through the SAME buffered
            // error-response body as a 413, so it exercises the identical flush-vs-teardown window
            // without flooding 64 MiB bodies (the S20 anti-pattern).
            let mut iter = w as u64;
            while !cancel.is_cancelled() {
                iter = iter.wrapping_add(1);
                // 0,1,2,4,8 ms cycle — 0ms drops before the head is read: the tightest race.
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
    // Race the error head against a tight teardown. Returns whether a clean head was seen.
    let saw_head = tokio::select! {
        biased;
        () = tokio::time::sleep(abort_after) => false,
        r = sender.send_request(req) => r.is_ok(),
    };
    drop(sender);
    driver.abort();
    Ok(saw_head)
}

/// H2 rapid-reset churn (CVE-2023-44487 accounting): open a stream and immediately abort it. The
/// bound under test: memory and the stream table must not grow unboundedly.
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
                let mut n = 0u32;
                while !cancel.is_cancelled() && n < 500 {
                    let mut s = sender.clone();
                    let req = Request::builder()
                        .method("GET")
                        .uri(format!("https://{sni}/"))
                        .body(Full::new(Bytes::new()));
                    let Ok(req) = req else { break };
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
                // HOLD the streams (do not await) to press the concurrent-stream cap.
                let mut holders = Vec::new();
                for _ in 0..300 {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let mut s = sender.clone();
                    let req = Request::builder()
                        .method("POST")
                        .uri(format!("https://{sni}/"))
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
