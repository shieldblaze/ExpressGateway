//! S15 A2 verify gate (vi) — strict_source_binding knob proved in BOTH positions per owner ruling
//! §9.1'.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_quic::public_header::MIN_INITIAL_DATAGRAM_BYTES;
use lb_quic::{PassthroughListener, PassthroughParams};
use lb_security::{RETRY_SECRET_LEN, RetryTokenSigner};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0xa5u8; RETRY_SECRET_LEN];

fn make_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lb-passthrough-ssb-{}-{}",
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
    let mut pkt = Vec::with_capacity(MIN_INITIAL_DATAGRAM_BYTES + token.len());
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
    // RFC 9000 §14.1: a client MUST expand every datagram carrying an Initial to at least 1200
    // bytes, and S47-QUIC-1 made the gateway enforce the matching server-side MUST ("discard an
    // Initial carried in a smaller datagram") — without which an 8-byte spoofed Initial drew a
    // ~96-byte Retry and the listener was a ~12x UDP reflector.
    //
    // These fixtures previously emitted ~22 bytes, which no conforming client could put on the
    // wire, so they are padded here rather than the gate being relaxed. A real client pads with
    // PADDING frames inside the encrypted payload; Mode A never decrypts, so trailing zeroes are
    // a faithful stand-in and leave every header field these tests assert on untouched.
    if pkt.len() < MIN_INITIAL_DATAGRAM_BYTES {
        pkt.resize(MIN_INITIAL_DATAGRAM_BYTES, 0u8);
    }
    pkt
}

/// Build a syntactically-valid QUIC v1 short-header datagram carrying `dcid` (whose length must
/// match the LB's `max_dcid_len_routed`).
fn build_short(dcid: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(2 + dcid.len());
    pkt.push(0b0100_0000);
    pkt.extend_from_slice(dcid);
    pkt.push(0xaa);
    pkt
}

/// Spawn a backend that counts datagrams received and exposes the count via an Arc<AtomicU64>.
async fn spawn_counting_backend() -> (SocketAddr, Arc<AtomicU64>) {
    let sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("bind counting backend");
    let addr = sock.local_addr().expect("local_addr");
    let count = Arc::new(AtomicU64::new(0));
    let count_for_task = Arc::clone(&count);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65_535];
        while sock.recv_from(&mut buf).await.is_ok() {
            count_for_task.fetch_add(1, Ordering::Relaxed);
        }
    });
    (addr, count)
}

async fn spawn_listener(
    strict: bool,
    short_dcid_len: usize,
) -> (
    PassthroughListener,
    SocketAddr,
    SocketAddr,
    Arc<AtomicU64>,
    CancellationToken,
) {
    let dir = make_dir();
    let retry_path = dir.join("retry.bin");
    std::fs::write(&retry_path, RETRY_SECRET).expect("write retry secret");
    let (backend, backend_count) = spawn_counting_backend().await;

    let mut params = PassthroughParams::new(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![backend],
        retry_path,
    );
    params.max_quic_connections = 256;
    params.min_client_dcid_len = 8;
    params.per_flow_backlog = 32;
    params.strict_source_binding = strict;
    params.max_dcid_len_routed = short_dcid_len;
    params.audit_throttle_window = Duration::from_secs(60);

    let cancel = CancellationToken::new();
    let listener = PassthroughListener::spawn(params, cancel.clone())
        .await
        .expect("spawn listener");
    let lb_addr = listener.local_addr();
    (listener, lb_addr, backend, backend_count, cancel)
}

/// Install one flow into the LB via a synthetic Retry-validated Initial from `client_a`.
async fn install_flow(
    lb: SocketAddr,
    client_a: &UdpSocket,
    signer: &RetryTokenSigner,
    dcid: &[u8],
) {
    let client_addr = client_a.local_addr().expect("local_addr");
    let token = signer.mint(client_addr, dcid);
    let scid = [0x55u8; 8];
    let pkt = build_initial(dcid, &scid, &token);
    let _ = client_a.send_to(&pkt, lb).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn ssb_false_accepts_nat_rebind() {
    const DCID_LEN: usize = 12;
    let dcid = [0xabu8; DCID_LEN];
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (listener, lb_addr, _backend, backend_count, cancel) =
        spawn_listener(false, DCID_LEN).await;

    let client_a = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client A bind");
    install_flow(lb_addr, &client_a, &signer, &dcid).await;
    let after_initial = backend_count.load(Ordering::Relaxed);

    let client_b = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client B bind");
    let short = build_short(&dcid);
    let _ = client_b.send_to(&short, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after_short = backend_count.load(Ordering::Relaxed);

    assert!(
        after_short > after_initial,
        "ssb=false: short-header from peer B was NOT forwarded \
         (before={after_initial} after={after_short}) — NAT rebind broken"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn ssb_true_drops_spoofed_source() {
    const DCID_LEN: usize = 12;
    let dcid = [0xcdu8; DCID_LEN];
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (listener, lb_addr, _backend, backend_count, cancel) = spawn_listener(true, DCID_LEN).await;

    let client_a = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client A bind");
    install_flow(lb_addr, &client_a, &signer, &dcid).await;
    let after_initial = backend_count.load(Ordering::Relaxed);

    let short = build_short(&dcid);
    let _ = client_a.send_to(&short, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after_short_a = backend_count.load(Ordering::Relaxed);
    assert!(
        after_short_a > after_initial,
        "ssb=true: short from ORIGINAL peer must still forward; \
         before={after_initial} after={after_short_a}"
    );

    let client_b = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client B bind");
    let _ = client_b.send_to(&short, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after_spoof = backend_count.load(Ordering::Relaxed);
    assert_eq!(
        after_spoof, after_short_a,
        "ssb=true: spoofed-source short-header was forwarded \
         (before={after_short_a} after={after_spoof}) — strict knob broken"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}
