//! S15 A3 verify gate 1 — spoofed-source-IP end-to-end (author != verifier).
//!
//! Installs a flow from peer A, then sends a spoofed short-header packet from peer B on the SAME
//! DCID with `strict_source_binding=true`, and asserts TWO layers: the spoofed packet is DROPPED
//! (backend datagram count unchanged) AND exactly one `audit/source_binding_violation` event is
//! emitted with the recorded+observed peers. A negative control (`strict=false`, packet FORWARDED,
//! NO audit line) proves the audit line is not vacuously emitted on every NAT-rebind.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use lb_quic::{PassthroughListener, PassthroughParams};
use lb_security::{RETRY_SECRET_LEN, RetryTokenSigner};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::field::{Field, Visit};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::Registry;

/// Requires the `audit/source_binding_violation` event to fire, not just the behavioural half
/// (backend not reached).
const AUDIT_LINE_REQUIRED: bool = true;

const AUDIT_TOKEN: &str = "source_binding_violation";

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0x7eu8; RETRY_SECRET_LEN];

#[derive(Clone, Default)]
struct AuditCounter {
    hits: Arc<AtomicUsize>,
}

impl AuditCounter {
    fn count(&self) -> usize {
        self.hits.load(Ordering::Acquire)
    }
}

struct TokenVisitor {
    found: bool,
}

impl Visit for TokenVisitor {
    fn record_debug(&mut self, _field: &Field, value: &dyn std::fmt::Debug) {
        if format!("{value:?}").contains(AUDIT_TOKEN) {
            self.found = true;
        }
    }

    fn record_str(&mut self, _field: &Field, value: &str) {
        if value.contains(AUDIT_TOKEN) {
            self.found = true;
        }
    }
}

impl<S> Layer<S> for AuditCounter
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut hit = meta.target().contains(AUDIT_TOKEN) || meta.name().contains(AUDIT_TOKEN);
        if !hit {
            let mut v = TokenVisitor { found: false };
            event.record(&mut v);
            hit = v.found;
        }
        if hit {
            self.hits.fetch_add(1, Ordering::AcqRel);
        }
    }
}

fn install_audit_capture() -> (AuditCounter, DefaultGuard) {
    let counter = AuditCounter::default();
    let subscriber = Registry::default().with(counter.clone());
    let guard = tracing::subscriber::set_default(subscriber);
    (counter, guard)
}

fn make_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lb-passthrough-spoof-{}-{}",
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

/// Short-header datagram carrying `dcid` (length == `max_dcid_len_routed`).
fn build_short(dcid: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(2 + dcid.len());
    pkt.push(0b0100_0000);
    pkt.extend_from_slice(dcid);
    pkt.push(0xaa);
    pkt
}

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
    (listener, lb_addr, backend_count, cancel)
}

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
async fn spoofed_source_dropped_and_audited() {
    const DCID_LEN: usize = 12;
    let dcid = [0xe1u8; DCID_LEN];
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (audit, _guard) = install_audit_capture();

    let (listener, lb_addr, backend_count, cancel) = spawn_listener(true, DCID_LEN).await;

    let client_a = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client A bind");
    install_flow(lb_addr, &client_a, &signer, &dcid).await;

    let short = build_short(&dcid);
    let _ = client_a.send_to(&short, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after_legit = backend_count.load(Ordering::Relaxed);
    let audit_after_legit = audit.count();
    assert_eq!(
        audit_after_legit, 0,
        "legit short from original peer must NOT emit a \
         source_binding_violation audit line (vacuous-audit guard)"
    );

    let client_b = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client B bind");
    let _ = client_b.send_to(&short, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after_spoof = backend_count.load(Ordering::Relaxed);
    let audit_after_spoof = audit.count();

    assert_eq!(
        after_spoof, after_legit,
        "spoofed-source short-header was FORWARDED to backend \
         (before={after_legit} after={after_spoof}) — strict \
         source-binding defence did NOT fire"
    );

    if AUDIT_LINE_REQUIRED {
        assert_eq!(
            audit_after_spoof, 1,
            "expected exactly ONE source_binding_violation audit line \
             for the spoofed packet, observed {audit_after_spoof}"
        );
    } else {
        eprintln!(
            "[gate1] AUDIT_LINE_REQUIRED=false; observed \
             source_binding_violation audit count = {audit_after_spoof} \
             (behavioral drop PASS; audit-line assertion deferred to \
             builder-1's A3 wiring)"
        );
    }

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn nonstrict_forwards_no_audit() {
    const DCID_LEN: usize = 12;
    let dcid = [0xe2u8; DCID_LEN];
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (audit, _guard) = install_audit_capture();

    let (listener, lb_addr, backend_count, cancel) = spawn_listener(false, DCID_LEN).await;

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
        "strict=false: different-source short-header must FORWARD \
         (before={after_initial} after={after_short}) — NAT-rebind broken"
    );
    assert_eq!(
        audit.count(),
        0,
        "strict=false must NOT emit a source_binding_violation audit \
         line on a normal NAT-rebind forward (non-vacuous matcher proof)"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}
