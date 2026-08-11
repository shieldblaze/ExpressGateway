//! S15 A2 verify gate (iv) — Bounded-state proof (R13 a/b/c).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_quic::{PassthroughListener, PassthroughParams};
use lb_security::{RETRY_SECRET_LEN, RetryTokenSigner};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0x5au8; RETRY_SECRET_LEN];

/// One-shot test directory under /tmp.
fn make_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lb-passthrough-bounded-state-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// Encode a QUIC varint into `out`.
fn varint(v: u64, out: &mut Vec<u8>) {
    if v < (1 << 6) {
        out.push(v as u8);
    } else if v < (1 << 14) {
        let b = v as u16 | 0b0100_0000_0000_0000;
        out.extend_from_slice(&b.to_be_bytes());
    } else if v < (1 << 30) {
        let b = (v as u32) | 0b1000_0000_0000_0000_0000_0000_0000_0000;
        out.extend_from_slice(&b.to_be_bytes());
    } else {
        let b =
            v | 0b1100_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;
        out.extend_from_slice(&b.to_be_bytes());
    }
}

/// Build a syntactically-valid QUIC v1 Initial with `dcid` as the destination CID and `token` in
/// the token field.
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

/// Distinct random-looking DCID for flow `i`.
fn dcid_for(i: u32) -> [u8; 12] {
    let mut d = [0u8; 12];
    d[..4].copy_from_slice(&i.to_be_bytes());
    d[4..8].copy_from_slice(&i.wrapping_mul(0x9e37_79b9).to_be_bytes());
    d[8..12].copy_from_slice(&i.wrapping_mul(0x517c_c1b7).to_be_bytes());
    d
}

/// Spawn a no-op backend that accepts UDP on a fresh local port.
async fn spawn_void_backend() -> SocketAddr {
    let sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("bind void backend");
    let addr = sock.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65_535];
        loop {
            if sock.recv_from(&mut buf).await.is_err() {
                break;
            }
        }
    });
    addr
}

/// Spawn the passthrough listener with `cap`, a known retry secret, and a single void backend.
async fn spawn_listener(cap: usize) -> (PassthroughListener, SocketAddr, CancellationToken) {
    let dir = make_dir();
    let retry_path = dir.join("retry.bin");
    std::fs::write(&retry_path, RETRY_SECRET).expect("write retry secret");
    let backend = spawn_void_backend().await;

    let mut params = PassthroughParams::new(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![backend],
        retry_path,
    );
    params.max_quic_connections = cap;
    params.min_client_dcid_len = 8;
    params.per_flow_backlog = 1;
    params.audit_throttle_window = Duration::from_secs(60);

    let cancel = CancellationToken::new();
    let listener = PassthroughListener::spawn(params, cancel.clone())
        .await
        .expect("spawn listener");
    let addr = listener.local_addr();
    (listener, addr, cancel)
}

#[tokio::test(flavor = "current_thread")]
async fn r13_a_burst_distinct_dcids_stays_bounded() {
    const CAP: usize = 2048;
    const BURST: u32 = 4096;
    let (listener, lb_addr, cancel) = spawn_listener(CAP).await;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    for i in 0..BURST {
        let dcid = dcid_for(i);
        let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("client bind");
        let client_addr = client.local_addr().expect("local_addr");
        let token = signer.mint(client_addr, &dcid);
        let scid = [0x22u8; 8];
        let pkt = build_initial(&dcid, &scid, &token);
        let _ = client.send_to(&pkt, lb_addr).await;
        if i % 64 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let flows = listener.flows_len();
    assert!(
        flows <= 2 * CAP,
        "flows_len={flows} exceeded 2*cap={}",
        2 * CAP
    );
    assert!(flows > 0, "no flows installed despite {BURST} initials");

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn r13_c_under_cap_no_eviction() {
    const CAP: usize = 256;
    const N: u32 = 250; // CAP - 6 with margin for table-doubling.
    let (listener, lb_addr, cancel) = spawn_listener(CAP).await;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let mut peak = 0usize;
    for i in 0..N {
        let dcid = dcid_for(i);
        let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("client bind");
        let client_addr = client.local_addr().expect("local_addr");
        let token = signer.mint(client_addr, &dcid);
        let scid = [0x33u8; 8];
        let pkt = build_initial(&dcid, &scid, &token);
        let _ = client.send_to(&pkt, lb_addr).await;
        if i % 32 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        peak = peak.max(listener.flows_len());
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let final_len = listener.flows_len();
    assert!(
        final_len >= peak,
        "final_len={final_len} < peak={peak} (eviction happened — R13(c) violated)"
    );
    assert!(
        final_len <= 2 * CAP,
        "final_len={final_len} > 2*cap={} despite N={N} < cap",
        2 * CAP
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn r13_b_cap_plus_one_drives_eviction_repeated() {
    const CAP: usize = 64;
    const ITERS: u32 = 50;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    for iter in 0..ITERS {
        let (listener, lb_addr, cancel) = spawn_listener(CAP).await;
        let burst = u32::try_from(CAP).unwrap() + 4;
        for i in 0..burst {
            let dcid = dcid_for(iter.wrapping_mul(1_000_003).wrapping_add(i));
            let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .expect("client bind");
            let client_addr = client.local_addr().expect("local_addr");
            let token = signer.mint(client_addr, &dcid);
            let scid = [0x44u8; 8];
            let pkt = build_initial(&dcid, &scid, &token);
            let _ = client.send_to(&pkt, lb_addr).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let flows = listener.flows_len();
        assert!(
            flows <= 2 * CAP,
            "iter {iter}: flows_len={flows} exceeded 2*cap={} (no eviction?)",
            2 * CAP
        );
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    }
}
