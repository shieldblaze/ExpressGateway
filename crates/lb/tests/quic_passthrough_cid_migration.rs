//! S15 A2 verify gate (iii) — CID-migration / NAT-rebind proof.
//!
//! Establish ONE passthrough flow (Initial with valid Retry token),
//! then have the client's UDP socket rebind to a NEW ephemeral source
//! port and continue sending short-header datagrams carrying the SAME
//! DCID. The LB MUST route those to the same backend (the flow stays
//! alive end-to-end), and `flows_len()` MUST stay bounded at the
//! single-flow level (`≤ 2` dispatch entries: client-DCID + backend-
//! SCID).
//!
//! This is the design §3.5 "Path migration (NAT rebind)" guarantee:
//! Table[dcid] still hits → packet routes to the same backend;
//! `FlowEntry.peer` updates on every recv → reverse-direction writes
//! follow the new peer.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use std::collections::HashSet;
use std::sync::Mutex;

use lb_quic::{PassthroughListener, PassthroughParams};
use lb_security::{RETRY_SECRET_LEN, RetryTokenSigner};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0xc3u8; RETRY_SECRET_LEN];

fn make_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lb-passthrough-cidmig-{}-{}",
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

fn build_short(dcid: &[u8], n: u8) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(2 + dcid.len());
    pkt.push(0b0100_0000);
    pkt.extend_from_slice(dcid);
    pkt.push(n);
    pkt
}

/// Backend that records distinct source-peer 4-tuples it ever
/// received from. After the LB-side per-flow socket connects, the LB
/// rebinds to ONE backend socket per flow — so the backend sees
/// exactly ONE source 4-tuple per flow, irrespective of how many
/// client-side ports the original sender used.
async fn spawn_unique_peer_backend() -> (SocketAddr, Arc<Mutex<HashSet<SocketAddr>>>, Arc<AtomicU64>)
{
    let sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("bind backend");
    let addr = sock.local_addr().expect("local_addr");
    let peers: Arc<Mutex<HashSet<SocketAddr>>> = Arc::new(Mutex::new(HashSet::new()));
    let count = Arc::new(AtomicU64::new(0));
    let peers_for_task = Arc::clone(&peers);
    let count_for_task = Arc::clone(&count);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65_535];
        while let Ok((_n, peer)) = sock.recv_from(&mut buf).await {
            peers_for_task.lock().unwrap().insert(peer);
            count_for_task.fetch_add(1, Ordering::Relaxed);
        }
    });
    (addr, peers, count)
}

async fn spawn_listener(
    short_dcid_len: usize,
) -> (
    PassthroughListener,
    SocketAddr,
    SocketAddr,
    Arc<Mutex<HashSet<SocketAddr>>>,
    Arc<AtomicU64>,
    CancellationToken,
) {
    let dir = make_dir();
    let retry_path = dir.join("retry.bin");
    std::fs::write(&retry_path, RETRY_SECRET).expect("write retry secret");
    let (backend, peers, count) = spawn_unique_peer_backend().await;
    let mut params = PassthroughParams::new(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![backend],
        retry_path,
    );
    params.max_quic_connections = 100;
    params.min_client_dcid_len = 8;
    params.per_flow_backlog = 32;
    params.strict_source_binding = false;
    params.max_dcid_len_routed = short_dcid_len;
    params.audit_throttle_window = Duration::from_secs(60);

    let cancel = CancellationToken::new();
    let listener = PassthroughListener::spawn(params, cancel.clone())
        .await
        .expect("spawn listener");
    let lb_addr = listener.local_addr();
    (listener, lb_addr, backend, peers, count, cancel)
}

#[tokio::test(flavor = "current_thread")]
async fn nat_rebind_preserves_single_flow() {
    const DCID_LEN: usize = 12;
    let dcid = [0xeeu8; DCID_LEN];
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);
    let (listener, lb_addr, _backend, backend_peers, backend_count, cancel) =
        spawn_listener(DCID_LEN).await;

    // ── Phase 1: install one flow via Initial from peer A. ──────
    let client_a = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client A bind");
    let client_a_addr = client_a.local_addr().expect("local_addr");
    let token = signer.mint(client_a_addr, &dcid);
    let scid = [0x66u8; 8];
    let pkt = build_initial(&dcid, &scid, &token);
    let _ = client_a.send_to(&pkt, lb_addr).await;

    // Some short-header packets through peer A (pre-migration).
    for n in 0..4u8 {
        let _ = client_a.send_to(&build_short(&dcid, n), lb_addr).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let flows_after_a = listener.flows_len();
    let count_after_a = backend_count.load(Ordering::Relaxed);
    assert!(
        (1..=2).contains(&flows_after_a),
        "post-phase-1 flows_len={flows_after_a} expected 1..=2 (single client-DCID key)"
    );
    assert!(
        count_after_a >= 5,
        "backend got count={count_after_a}, expected ≥5 (1 initial + 4 short)"
    );

    // ── Phase 2: NAT rebind — close client A, open client B
    // (different ephemeral port), continue sending SAME DCID. ───
    drop(client_a);
    let client_b = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client B bind");
    let client_b_addr = client_b.local_addr().expect("local_addr");
    assert_ne!(
        client_b_addr, client_a_addr,
        "expected fresh ephemeral port for client B"
    );

    for n in 16..20u8 {
        let _ = client_b.send_to(&build_short(&dcid, n), lb_addr).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let flows_after_b = listener.flows_len();
    let count_after_b = backend_count.load(Ordering::Relaxed);
    assert_eq!(
        flows_after_b, flows_after_a,
        "NAT rebind created a new dispatch entry: {flows_after_a} → {flows_after_b} \
         (expected the same flow to handle the rebind)"
    );
    assert!(
        count_after_b > count_after_a,
        "post-rebind: backend count={count_after_b} did not grow past {count_after_a}; \
         rebind packets dropped"
    );

    // Backend sees exactly ONE peer 4-tuple — the LB's per-flow
    // backend socket — irrespective of how many client-side ports
    // the original sender used. This is by design §3.4 (the per-flow
    // backend socket isolates the backend from client-side NAT
    // shenanigans).
    let peers_snapshot: Vec<SocketAddr> = backend_peers.lock().unwrap().iter().copied().collect();
    assert_eq!(
        peers_snapshot.len(),
        1,
        "backend should see exactly 1 source peer (the LB's per-flow socket); \
         saw {}: {:?}",
        peers_snapshot.len(),
        peers_snapshot
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}
