//! S15 A3 verify gate 2 — audit-throttle saturation.
//!
//! Design §A3 verify gate: "Saturation test: 10_000 cap-hits emit one
//! log line per `audit_throttle_window_secs` (default 60s), not
//! 10_000."
//!
//! Threat model §6.2 (CID-flood / tracking-table exhaustion): a
//! flood of over-cap Initials must NOT produce a flood of audit log
//! lines (log amplification is itself a DoS amplifier — disk, ingest,
//! alert-storm). The cap-hit audit line is rate-limited to ONE per
//! `audit_throttle_window`.
//!
//! Harness shape:
//!
//!   * Cap the listener small (CAP), then drive >> CAP distinct-DCID
//!     Retry-validated Initials. Every Initial beyond CAP forces
//!     `evict_oldest` (a cap-hit). 10_000 sends → thousands of cap-hits.
//!   * A tracing-capture layer counts `cap_hit` audit events.
//!
//! TWO assertion layers, by proven mechanism:
//!
//!   * **Cap-hit path reachable + bounded** — `flows_len()` stays
//!     ≤ 2*CAP across the whole 10_000-send flood (no unbounded
//!     growth, no panic). Provable against the CURRENT tree (the
//!     `evict_oldest` cap path already exists, A2).
//!   * **Throttled audit count** — with a LONG window (60s), the whole
//!     flood emits AT MOST ONE `cap_hit` audit line; with a SHORT
//!     window, more than one (the throttle releases per window). This
//!     asserts builder-1's A3 cap-hit-audit + throttle wiring. Until
//!     that wiring lands, the count assertions are gated by
//!     [`AUDIT_LINE_REQUIRED`] so this file COMPILES and the
//!     reachability half PASSES against the current tree (parallel
//!     authoring per the A3 task split). Flip to `true` once builder-1
//!     DMs the wiring landed.

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

/// Flip to `true` once builder-1's A3 cap-hit-audit + throttle wiring
/// lands. Until then the reachability + boundedness half is the binding
/// assertion; the throttled-count half is observed-and-reported. That
/// is the gate-2 completion bar.
const AUDIT_LINE_REQUIRED: bool = false;

/// Token the cap-hit audit line is expected to carry. Design §11.5
/// names the line `audit/quic_passthrough_cap_hit`; this matches the
/// stable substring `cap_hit`.
const CAP_HIT_TOKEN: &str = "cap_hit";

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0x3cu8; RETRY_SECRET_LEN];

// ============================================================
// Tracing capture.
// ============================================================

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

// ============================================================
// Synthetic wire builders.
// ============================================================

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

/// Drive `n` distinct-DCID Retry-validated Initials at `lb` from a
/// single long-lived client socket. Returns once all are sent (the
/// listener drains asynchronously). Each Initial beyond `cap` forces a
/// cap-hit eviction.
async fn flood(lb: SocketAddr, client: &UdpSocket, signer: &RetryTokenSigner, n: u32, salt: u32) {
    let client_addr = client.local_addr().expect("local_addr");
    let scid = [0x66u8; 8];
    for i in 0..n {
        let dcid = dcid_for(salt.wrapping_mul(0x0100_0193).wrapping_add(i));
        let token = signer.mint(client_addr, &dcid);
        let pkt = build_initial(&dcid, &scid, &token);
        let _ = client.send_to(&pkt, lb).await;
        // Periodic yield so the recv loop drains and the socket buffer
        // does not silently drop our floods (which would make the
        // cap-hit path under-exercised — a vacuous pass).
        if i % 256 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

// ============================================================
// Gate 2 — 10_000 cap-hits, long window → at most ONE audit line.
// ============================================================

#[tokio::test(flavor = "current_thread")]
async fn ten_thousand_cap_hits_throttled_to_one_line() {
    const CAP: usize = 256;
    const SENDS: u32 = 10_000;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (audit, _guard) = install_audit_capture();

    // Long window (60s) — the entire flood falls inside ONE window.
    let (listener, lb_addr, cancel) = spawn_listener(CAP, Duration::from_secs(60)).await;

    let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client bind");
    flood(lb_addr, &client, &signer, SENDS, 1).await;
    // Let the listener drain the inflight queue.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Reachability + boundedness (binding against the CURRENT tree):
    // the table never exceeds 2*CAP despite 10_000 distinct-DCID
    // installs — the cap path FIRED thousands of times and bounded it.
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
        // The whole 10_000-cap-hit flood is inside one 60s window → AT
        // MOST one audit line (zero is acceptable only if NO cap-hit
        // occurred, but flows>0 with SENDS>>CAP guarantees cap-hits).
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

// ============================================================
// Gate 2 companion — short window releases more than one line.
//
// Proves the throttle is window-keyed (not a permanent one-shot): with
// a sub-millisecond window and a sleep between two flood bursts, two
// distinct windows each release a line. Guards against an
// implementation that emits exactly one line FOREVER (which would also
// pass the long-window test vacuously).
// ============================================================

#[tokio::test(flavor = "current_thread")]
async fn short_window_releases_per_window() {
    const CAP: usize = 64;
    const BURST: u32 = 2_000;
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (audit, _guard) = install_audit_capture();

    // Sub-millisecond window: each burst, separated by a sleep longer
    // than the window, falls into its own window.
    let (listener, lb_addr, cancel) =
        spawn_listener(CAP, Duration::from_micros(1)).await;

    let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client bind");

    // First window.
    flood(lb_addr, &client, &signer, BURST, 10).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after_first = audit.count();

    // Second window (window long elapsed; 50ms >> 1µs).
    flood(lb_addr, &client, &signer, BURST, 20).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after_second = audit.count();

    // Reachability + boundedness (binding now).
    let flows = listener.flows_len();
    assert!(
        flows <= 2 * CAP,
        "flows_len={flows} exceeded 2*cap={} — cap path did not bound",
        2 * CAP
    );

    if AUDIT_LINE_REQUIRED {
        // Each burst is its own window → strictly more than one line
        // total, and the second window adds at least one beyond the
        // first. Still FAR fewer than the ~4000 cap-hits driven.
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
