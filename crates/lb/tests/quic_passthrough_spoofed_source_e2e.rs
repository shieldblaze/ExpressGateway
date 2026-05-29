//! S15 A3 verify gate 1 — spoofed-source-IP end-to-end.
//!
//! Independent verification (author≠verifier): builder-1 writes the
//! impl; this file is the verifier's INDEPENDENT spoofed-source e2e.
//!
//! Design §A3 verify gate: "End-to-end: spoofed-source-IP test
//! (in-process socket-pair fixture) hits source-binding defence and is
//! dropped with `audit/source_binding_violation` log line."
//!
//! Fixture shape (matches the A2
//! `quic_passthrough_strict_source_binding.rs` style):
//!
//!   1. Install a flow from peer A (Retry-validated synthetic Initial).
//!   2. Send a spoofed short-header packet from peer B on the SAME DCID
//!      with `strict_source_binding=true`.
//!   3. Assert the spoofed packet is DROPPED (never reaches the
//!      backend) AND an `audit/source_binding_violation` event is
//!      emitted on the tracing pipeline.
//!
//! TWO assertion layers, by proven mechanism:
//!
//!   * **Behavioral drop** — backend datagram count is unchanged by the
//!     spoofed packet (the defence FIRED). This is provable against the
//!     CURRENT tree (the `forward_short_via` strict gate already exists,
//!     A2).
//!   * **Audit log line** — exactly one `audit/source_binding_violation`
//!     event with the recorded+observed peers. This asserts builder-1's
//!     A3 audit-line wiring. Until that wiring lands, the assertion is
//!     gated by [`AUDIT_LINE_REQUIRED`] below so this file COMPILES and
//!     the behavioral half PASSES against the current tree (the
//!     verifier authors gates 1+2 in parallel with builder-1 per the
//!     A3 task split). Flip [`AUDIT_LINE_REQUIRED`] to `true` once
//!     builder-1 DMs that the audit line landed; that is the gate-1
//!     completion bar.
//!
//! A negative control (`strict=false`, spoofed packet FORWARDED, NO
//! audit line) proves the audit line is not vacuously emitted on every
//! NAT-rebind.

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

/// Flip to `true` once builder-1's A3 audit-line wiring lands (the
/// `audit/source_binding_violation` event). Until then the behavioral
/// half (backend not reached) is the binding assertion; the audit-line
/// half is observed-and-reported but not asserted, so the verifier can
/// author this gate in parallel with the impl per the A3 task split.
///
/// When `true`, the test additionally REQUIRES the audit event to fire.
const AUDIT_LINE_REQUIRED: bool = false;

/// The audit event the design §A3 names. Builder-1's wiring is expected
/// to emit a tracing event whose target OR message carries this token.
const AUDIT_TOKEN: &str = "source_binding_violation";

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0x7eu8; RETRY_SECRET_LEN];

// ============================================================
// Tracing capture — count audit events by token match.
// ============================================================

/// Counts tracing events whose target or message contains `AUDIT_TOKEN`.
/// Shared via Arc so the test thread reads the count after the
/// listener has processed the spoofed packet.
#[derive(Clone, Default)]
struct AuditCounter {
    hits: Arc<AtomicUsize>,
}

impl AuditCounter {
    fn count(&self) -> usize {
        self.hits.load(Ordering::Acquire)
    }
}

/// Visitor that scans event fields for `AUDIT_TOKEN` (covers the case
/// where the audit marker is in the `message` field rather than the
/// event target/name).
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
        // Match the audit token in the event target, the event name, or
        // any field value (covers `audit/source_binding_violation` as a
        // target, a message, or a field). Builder-1's exact shape is not
        // yet pinned; this matcher is deliberately broad so it observes
        // whatever form the wiring takes, and the negative control
        // proves it is not over-broad (no hit on the non-strict path).
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

/// Install a thread-local tracing subscriber for the duration of the
/// returned guard, returning the shared audit counter. Thread-local
/// (`set_default`) so parallel tests do not collide on the global
/// default — each `#[tokio::test(flavor = "current_thread")]` runs on
/// one thread and the listener's recv loop runs on that same runtime.
fn install_audit_capture() -> (AuditCounter, DefaultGuard) {
    let counter = AuditCounter::default();
    let subscriber = Registry::default().with(counter.clone());
    let guard = tracing::subscriber::set_default(subscriber);
    (counter, guard)
}

// ============================================================
// Synthetic wire builders (match the A2 SSB test).
// ============================================================

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

/// Install one flow into the LB via a synthetic Retry-validated Initial
/// from `client_a`. The DCID becomes the short-header routing key.
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

// ============================================================
// Gate 1 — spoofed-source e2e (strict=true): DROP + audit line.
// ============================================================

#[tokio::test(flavor = "current_thread")]
async fn spoofed_source_dropped_and_audited() {
    const DCID_LEN: usize = 12;
    let dcid = [0xe1u8; DCID_LEN];
    let signer = RetryTokenSigner::new_with_secret(RETRY_SECRET);

    let (audit, _guard) = install_audit_capture();

    let (listener, lb_addr, backend_count, cancel) = spawn_listener(true, DCID_LEN).await;

    // Peer A installs the flow.
    let client_a = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client A bind");
    install_flow(lb_addr, &client_a, &signer, &dcid).await;

    // Control: short-header from the ORIGINAL peer A is forwarded and
    // does NOT emit a source-binding-violation audit line.
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

    // Spoof: short-header from peer B (different source 4-tuple) on the
    // SAME DCID. Must be DROPPED at the LB (backend count unchanged) and
    // emit exactly one source_binding_violation audit line.
    let client_b = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .expect("client B bind");
    let _ = client_b.send_to(&short, lb_addr).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after_spoof = backend_count.load(Ordering::Relaxed);
    let audit_after_spoof = audit.count();

    // Behavioral assertion (binding against the CURRENT tree): the
    // spoofed packet did not reach the backend.
    assert_eq!(
        after_spoof, after_legit,
        "spoofed-source short-header was FORWARDED to backend \
         (before={after_legit} after={after_spoof}) — strict \
         source-binding defence did NOT fire"
    );

    // Audit-line assertion (binding once builder-1's wiring lands).
    if AUDIT_LINE_REQUIRED {
        assert_eq!(
            audit_after_spoof, 1,
            "expected exactly ONE source_binding_violation audit line \
             for the spoofed packet, observed {audit_after_spoof}"
        );
    } else {
        // Observe-and-report mode (parallel authoring). The behavioral
        // half is the binding gate until the wiring lands.
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

// ============================================================
// Negative control — strict=false: FORWARD + NO audit line.
// ============================================================

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

    // Different-source short-header under strict=false: FORWARDED
    // (NAT-rebind accommodation), NO source-binding-violation audit.
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
