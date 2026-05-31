//! The observability loop: every `interval`, sample the gateway child's
//! `/proc` footprint (RSS/fd/threads) and scrape its `/metrics` for the
//! scenario's state-table gauges, append a row to a [`TimeSeries`], and print a
//! one-line heartbeat snapshot (the soak's liveness signal — R9: a quiet soak
//! is expected; the SNAPSHOT stream, not the agent, is the heartbeat).

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::metrics::{self, MetricSet};
use crate::procstat;
use crate::timeseries::TimeSeries;

/// The fixed leading columns (OS footprint), before the per-scenario gauges.
pub const BASE_COLUMNS: [&str; 4] = ["rss_kb", "vmhwm_kb", "fds", "threads"];

/// Minimal HTTP GET over a fresh TCP connection (HTTP/1.1 + `Connection:
/// close`, read to EOF). Returns `(status, body)`. Used for the gateway
/// readiness gate and the `/metrics` scrape — no hyper client churn in the hot
/// sampling loop.
pub async fn http_get(addr: SocketAddr, path: &str) -> anyhow::Result<(u16, String)> {
    let mut stream =
        tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(addr)).await??;
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::with_capacity(64 * 1024);
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf)).await??;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    Ok((status, body.to_string()))
}

/// Scrape `/metrics` and parse it. Returns an empty set on any failure (a
/// transient scrape miss must not kill the sampler — the row records NaN).
pub async fn scrape(metrics_addr: SocketAddr) -> MetricSet {
    match http_get(metrics_addr, "/metrics").await {
        Ok((200..=299, body)) => metrics::parse(&body),
        _ => MetricSet::default(),
    }
}

/// Run the sampling loop until `cancel` fires (plus one final post-cancel
/// sample so the drained-down state is recorded). Returns the full series.
///
/// `gauges` is the ordered list of `/metrics` family names to track (each
/// summed across label sets). The series columns are [`BASE_COLUMNS`] followed
/// by `gauges`.
pub async fn run_sampler(
    pid: u32,
    metrics_addr: SocketAddr,
    gauges: Vec<String>,
    interval: Duration,
    cancel: CancellationToken,
    label: &str,
) -> TimeSeries {
    let mut columns: Vec<String> = BASE_COLUMNS.iter().map(|s| (*s).to_string()).collect();
    columns.extend(gauges.iter().cloned());
    let mut ts = TimeSeries::new(columns);

    let start = tokio::time::Instant::now();
    loop {
        let t = start.elapsed().as_secs_f64();
        let row = build_row(pid, metrics_addr, &gauges).await;
        print_heartbeat(label, t, &gauges, &row);
        ts.push(t, row);

        // Stop after recording one sample post-cancel.
        if cancel.is_cancelled() {
            break;
        }
        tokio::select! {
            () = cancel.cancelled() => {
                // Take one more (final) sample on the next loop iteration.
            }
            () = tokio::time::sleep(interval) => {}
        }
    }
    ts
}

/// Build a single sample row: [rss, vmhwm, fds, threads, gauge values…].
async fn build_row(pid: u32, metrics_addr: SocketAddr, gauges: &[String]) -> Vec<f64> {
    let fp = procstat::sample_pid(pid);
    let metricset = scrape(metrics_addr).await;
    let mut row = match fp {
        Some(fp) => vec![
            fp.rss_kb as f64,
            fp.vmhwm_kb as f64,
            fp.fds as f64,
            fp.threads as f64,
        ],
        None => vec![f64::NAN, f64::NAN, f64::NAN, f64::NAN],
    };
    for g in gauges {
        row.push(metricset.sum(g).unwrap_or(f64::NAN));
    }
    row
}

fn print_heartbeat(label: &str, t: f64, gauges: &[String], row: &[f64]) {
    let rss_mb = row.first().copied().unwrap_or(f64::NAN) / 1024.0;
    let fds = row.get(2).copied().unwrap_or(f64::NAN);
    let threads = row.get(3).copied().unwrap_or(f64::NAN);
    let mut extra = String::new();
    for (i, g) in gauges.iter().enumerate() {
        let v = row.get(4 + i).copied().unwrap_or(f64::NAN);
        if v.is_nan() {
            extra.push_str(&format!(" {g}=-"));
        } else {
            extra.push_str(&format!(" {g}={v:.0}"));
        }
    }
    println!(
        "[{label}] t={t:>6.0}s rss={rss_mb:>7.1}MB fds={fds:>4.0} thr={threads:>3.0} |{extra}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn http_get_against_dead_port_errors() {
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert!(http_get(addr, "/metrics").await.is_err());
    }

    #[tokio::test]
    async fn scrape_dead_port_is_empty_set() {
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let m = scrape(addr).await;
        assert!(
            m.samples.is_empty(),
            "failed scrape yields empty set, not a panic"
        );
    }

    #[tokio::test]
    async fn http_get_parses_status_and_body() {
        // Stand up a one-shot TCP server returning a tiny HTTP/1.1 response.
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = listener.accept().await {
                let mut scratch = [0u8; 1024];
                let _ = s.read(&mut scratch).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .await;
            }
        });
        let (status, body) = http_get(addr, "/metrics").await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "hello");
    }
}
