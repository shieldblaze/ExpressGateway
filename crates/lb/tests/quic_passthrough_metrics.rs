//! S15 A3 verify gate 3 — `quic_passthrough_*` metrics.
//!
//! Independent verification (author≠verifier): builder-1 added the
//! `lb-observability::PassthroughMetrics` family + the bump sites in
//! `lb-quic::passthrough`, and a UNIT suite inside `passthrough.rs`
//! driving the router ctx directly. This file is the verifier's
//! INDEPENDENT integration-level check: it drives the REAL
//! `PassthroughListener` over the loopback UDP datapath and asserts the
//! metric handles, read back from the shared `MetricsRegistry`,
//! increment correctly.
//!
//! Design §A3 verify gate: "Metrics assertions — assert they increment
//! correctly (flows, flows_evicted_total, retry_minted_total,
//! retry_rejected_total, header_parse_errors_total,
//! backend_socket_errors_total) via the metrics registry."
//!
//! Gauge semantics (lead Q2 ruling): `quic_passthrough_flows` is
//! `.set(table.len())` — it tracks DISPATCH-TABLE size, so a migrated
//! flow that holds two CID keys reads 2. We assert it tracks
//! `flows_len()` (the same dispatch-table count), NOT `== 1`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_observability::{MetricsRegistry, PassthroughMetrics};
use lb_quic::{PassthroughListener, PassthroughParams};
use lb_security::{RETRY_SECRET_LEN, RetryTokenSigner};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0x9bu8; RETRY_SECRET_LEN];

fn make_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lb-passthrough-metrics-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn varint(v: u64, out: &mut Vec<u8>) {
    if v < (1 << 6) {
        out.push(v as u8);
    } else if v < (1 << 14) {
        out.extend_from_slice(&(v as u16 | 0x4000).to_be_bytes());
    } else if v < (1 << 30) {
        out.extend_from_slice(&((v as u32) | 0x8000_0000).to_be_bytes());
    } else {
        out.extend_from_slice(&(v | 0xc000_0000_0000_0000).to_be_bytes());
    }
}

fn build_initial(dcid: &[u8], scid: &[u8], token: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(64 + token.len());
    pkt.push(0b1100_0000);
    pkt.extend_from_slice(&0x0000_0001u32.to_be_bytes());
    pkt.push(u8::try_from(dcid.len()).unwrap());
    pkt.extend_from_slice(dcid);
    pkt.push(u8::try_from(scid.len()).unwrap());
    pkt.extend_from_slice(scid);
    varint(token.len() as u64, &mut pkt);
    pkt.extend_from_slice(token);
    varint(1, &mut pkt);
    pkt.push(0u8);
    pkt
}

/// A truncated long header — DCID length byte claims 20 bytes but the
/// datagram ends early. The public-header parser must reject it,
/// bumping `header_parse_errors_total`.
fn build_truncated_long() -> Vec<u8> {
    vec![
        0b1100_0000, // long header, fixed bit, Initial
        0x00,
        0x00,
        0x00,
        0x01, // version 1
        20,   // DCID len 20 — but no DCID bytes follow → Truncated
    ]
}

fn dcid_for(i: u32) -> [u8; 12] {
    let mut d = [0u8; 12];
    d[..4].copy_from_slice(&i.to_be_bytes());
    d[4..8].copy_from_slice(&i.wrapping_mul(0x9e37_79b9).to_be_bytes());
    d[8..12].copy_from_slice(&i.wrapping_mul(0x517c_c1b7).to_be_bytes());
    d
}

async fn spawn_void_backend() -> SocketAddr {
    let sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("bind void backend");
    let addr = sock.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65_535];
        while sock.recv_from(&mut buf).await.is_ok() {}
    });
    addr
}

/// Spawn a listener with a fresh metrics registry wired in. Returns the
/// listener, its addr, the metrics handles (read-back surface), and the
/// cancel token.
async fn spawn_with_metrics(
    cap: usize,
    mint_retry: bool,
) -> (
    PassthroughListener,
    SocketAddr,
    PassthroughMetrics,
    CancellationToken,
) {
    let dir = make_dir();
    let retry_path = dir.join("retry.bin");
    std::fs::write(&retry_path, RETRY_SECRET).expect("write retry secret");
    let backend = spawn_void_backend().await;

    let registry = MetricsRegistry::new();
    let metrics = PassthroughMetrics::register(&registry).expect("register metrics");

    let mut params = PassthroughParams::new(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![backend],
        retry_path,
    );
    params.max_quic_connections = cap;
    params.min_client_dcid_len = 8;
    params.per_flow_backlog = 4;
    params.mint_retry = mint_retry;
    params.audit_throttle_window = Duration::from_secs(60);
    params.metrics = Some(metrics.clone());

    let cancel = CancellationToken::new();
    let listener = PassthroughListener::spawn(params, cancel.clone())
        .await
        .expect("spawn listener");
    let addr = listener.local_addr();
    (listener, addr, metrics, cancel)
}

// ============================================================
// flows gauge + retry_minted + retry_rejected.
// ============================================================

#[tokio::test(flavor = "current_thread")]
async fn flows_gauge_and_retry_counters() {
    // mint_retry=true: a no-token Initial mints a Retry (no flow yet).
    // A Retry-validated Initial installs a flow. A bad-token Initial is
    // rejected.
    const CAP: usize = 256;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);
    let (listener, lb_addr, m, cancel) = spawn_with_metrics(CAP, true).await;

    let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client bind");
    let client_addr = client.local_addr().expect("local_addr");
    let scid = [0x11u8; 8];

    // (1) No-token Initial → Retry minted, no flow installed.
    let dcid0 = dcid_for(1);
    let no_token = build_initial(&dcid0, &scid, &[]);
    let _ = client.send_to(&no_token, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        m.retry_minted_total.get() >= 1,
        "no-token Initial must mint a Retry (retry_minted_total={})",
        m.retry_minted_total.get()
    );
    assert_eq!(
        m.flows.get(),
        0,
        "no-token Initial must NOT install a flow (flows={})",
        m.flows.get()
    );

    // (2) Retry-validated Initial → one flow installed; gauge tracks the
    // dispatch-table size (== flows_len), per the Q2 gauge ruling.
    let dcid1 = dcid_for(2);
    let token = signer.mint(client_addr, &dcid1);
    let good = build_initial(&dcid1, &scid, &token);
    let _ = client.send_to(&good, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let gauge = m.flows.get();
    let table = i64::try_from(listener.flows_len()).unwrap();
    assert!(gauge > 0, "flows gauge did not move after a valid Initial");
    assert_eq!(
        gauge, table,
        "flows gauge ({gauge}) must equal dispatch-table size \
         flows_len() ({table}) — Q2 .set(table.len()) semantics"
    );

    // (3) Bad-token Initial (token minted for a DIFFERENT peer) →
    // rejected, retry_rejected_total bumps, no new flow.
    let dcid2 = dcid_for(3);
    let wrong_peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 9, 8, 7)), 4321);
    let bad_token = signer.mint(wrong_peer, &dcid2);
    let before_rej = m.retry_rejected_total.get();
    let before_flows = m.flows.get();
    let bad = build_initial(&dcid2, &scid, &bad_token);
    let _ = client.send_to(&bad, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        m.retry_rejected_total.get(),
        before_rej + 1,
        "bad-token Initial must bump retry_rejected_total exactly once"
    );
    assert_eq!(
        m.flows.get(),
        before_flows,
        "bad-token Initial must NOT install a flow"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}

// ============================================================
// header_parse_errors_total.
// ============================================================

#[tokio::test(flavor = "current_thread")]
async fn header_parse_errors_counter() {
    let (listener, lb_addr, m, cancel) = spawn_with_metrics(256, true).await;
    let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client bind");

    assert_eq!(m.header_parse_errors_total.get(), 0, "starts at zero");

    // Send three malformed datagrams (truncated long headers).
    for _ in 0..3 {
        let _ = client.send_to(&build_truncated_long(), lb_addr).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        m.header_parse_errors_total.get(),
        3,
        "three malformed datagrams must bump header_parse_errors_total \
         to exactly 3 (observed {})",
        m.header_parse_errors_total.get()
    );
    // No flow installed; no Retry minted on a parse error.
    assert_eq!(m.flows.get(), 0, "parse error must not install a flow");

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}

// ============================================================
// flows_evicted_total + gauge tracks eviction.
// ============================================================

#[tokio::test(flavor = "current_thread")]
async fn flows_evicted_counter_and_gauge_bounded() {
    // Drive cap+overflow distinct flows; evictions must be counted and
    // the gauge must stay bounded at the dispatch-table size.
    const CAP: usize = 32;
    const SENDS: u32 = 200;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);
    let (listener, lb_addr, m, cancel) = spawn_with_metrics(CAP, true).await;

    let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client bind");
    let client_addr = client.local_addr().expect("local_addr");
    let scid = [0x22u8; 8];

    for i in 0..SENDS {
        let dcid = dcid_for(1000 + i);
        let token = signer.mint(client_addr, &dcid);
        let pkt = build_initial(&dcid, &scid, &token);
        let _ = client.send_to(&pkt, lb_addr).await;
        if i % 16 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    // SENDS (200) >> CAP (32) so eviction MUST have fired.
    assert!(
        m.flows_evicted_total.get() >= 1,
        "cap+overflow drove no evictions (flows_evicted_total={})",
        m.flows_evicted_total.get()
    );
    // Gauge tracks the bounded dispatch-table size == flows_len().
    let gauge = m.flows.get();
    let table = i64::try_from(listener.flows_len()).unwrap();
    assert_eq!(
        gauge, table,
        "flows gauge ({gauge}) must equal flows_len() ({table}) after \
         eviction churn"
    );
    assert!(
        gauge <= i64::try_from(2 * CAP).unwrap(),
        "flows gauge ({gauge}) exceeded 2*cap ({}) — gauge not bounded \
         by eviction",
        2 * CAP
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}
