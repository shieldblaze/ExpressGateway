//! S15 A3 verify gate 2 — audit-throttle saturation.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use lb_quic::public_header::MIN_INITIAL_DATAGRAM_BYTES;
use lb_quic::{PassthroughListener, PassthroughParams};
use lb_security::{RETRY_SECRET_LEN, RetryTokenSigner};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::field::{Field, Visit};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::Registry;

/// Requires the cap-hit audit + throttle counters to be asserted. Landed at integration tip
/// `b8499ea2`, so the count assertions are binding.
const AUDIT_LINE_REQUIRED: bool = true;

/// Token the cap-hit audit line is expected to carry.
const CAP_HIT_TOKEN: &str = "cap_hit";

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0x3cu8; RETRY_SECRET_LEN];

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
        if format!("{value:?}").contains(CAP_HIT_TOKEN) {
            self.found = true;
        }
    }

    fn record_str(&mut self, _field: &Field, value: &str) {
        if value.contains(CAP_HIT_TOKEN) {
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
        let mut hit = meta.target().contains(CAP_HIT_TOKEN) || meta.name().contains(CAP_HIT_TOKEN);
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
        "lb-passthrough-throttle-{}-{}",
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

/// Distinct DCID for flow `i` (12 bytes — above the min floor of 8).
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

async fn spawn_listener(
    cap: usize,
    window: Duration,
) -> (PassthroughListener, SocketAddr, CancellationToken) {
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
    params.audit_throttle_window = window;

    let cancel = CancellationToken::new();
    let listener = PassthroughListener::spawn(params, cancel.clone())
        .await
        .expect("spawn listener");
    let addr = listener.local_addr();
    (listener, addr, cancel)
}

/// Drive `n` distinct-DCID Retry-validated Initials at `lb` from a single long-lived client socket.
async fn flood(lb: SocketAddr, client: &UdpSocket, signer: &RetryTokenSigner, n: u32, salt: u32) {
    let client_addr = client.local_addr().expect("local_addr");
    let scid = [0x66u8; 8];
    for i in 0..n {
        let dcid = dcid_for(salt.wrapping_mul(0x0100_0193).wrapping_add(i));
        let token = signer.mint(client_addr, &dcid);
        let pkt = build_initial(&dcid, &scid, &token);
        let _ = client.send_to(&pkt, lb).await;
        if i % 256 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ten_thousand_cap_hits_throttled_to_one_line() {
    const CAP: usize = 256;
    const SENDS: u32 = 10_000;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (audit, _guard) = install_audit_capture();

    let (listener, lb_addr, cancel) = spawn_listener(CAP, Duration::from_secs(60)).await;

    let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client bind");
    flood(lb_addr, &client, &signer, SENDS, 1).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let flows = listener.flows_len();
    assert!(
        flows <= 2 * CAP,
        "flows_len={flows} exceeded 2*cap={} after {SENDS} sends — \
         cap-hit eviction did not bound the table",
        2 * CAP
    );
    assert!(
        flows > 0,
        "no flows installed despite {SENDS} valid Initials — flood was \
         dropped at the socket (cap-hit path under-exercised → gate would \
         be vacuous)"
    );

    let observed = audit.count();
    if AUDIT_LINE_REQUIRED {
        assert_eq!(
            observed, 1,
            "expected exactly ONE cap_hit audit line for {SENDS} sends \
             in a single 60s window, observed {observed} — throttle \
             broken (or unthrottled flood)"
        );
    } else {
        eprintln!(
            "[gate2] AUDIT_LINE_REQUIRED=false; {SENDS} sends, \
             flows_len={flows} (<=2*cap), observed cap_hit audit count = \
             {observed} (reachability+boundedness PASS; throttled-count \
             assertion deferred to builder-1's A3 wiring)"
        );
    }

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn short_window_releases_per_window() {
    const CAP: usize = 64;
    const BURST: u32 = 2_000;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (audit, _guard) = install_audit_capture();

    let (listener, lb_addr, cancel) = spawn_listener(CAP, Duration::from_micros(1)).await;

    let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client bind");

    flood(lb_addr, &client, &signer, BURST, 10).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after_first = audit.count();

    flood(lb_addr, &client, &signer, BURST, 20).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after_second = audit.count();

    let flows = listener.flows_len();
    assert!(
        flows <= 2 * CAP,
        "flows_len={flows} exceeded 2*cap={} — cap path did not bound",
        2 * CAP
    );

    if AUDIT_LINE_REQUIRED {
        assert!(
            after_second > after_first,
            "short-window throttle did not release a NEW cap_hit line in \
             the second window (after_first={after_first} \
             after_second={after_second}) — throttle is a permanent \
             one-shot, not window-keyed"
        );
        assert!(
            after_second < 2 * BURST as usize,
            "short-window emitted {after_second} lines for {} cap-hits — \
             not throttled at all",
            2 * BURST
        );
    } else {
        eprintln!(
            "[gate2-short] AUDIT_LINE_REQUIRED=false; after_first={after_first} \
             after_second={after_second} flows_len={flows} (reachability \
             PASS; per-window release assertion deferred)"
        );
    }

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}
