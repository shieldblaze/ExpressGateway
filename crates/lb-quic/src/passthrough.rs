//! Mode A passthrough datapath (S15 A2) — `audit/quic/s15-design.md`
//! §3 + §5 + §A2.
//!
//! The LB routes encrypted QUIC packets by Connection ID without
//! decrypting. There is NO TLS state on this path:
//!
//! - No quiche::Connection ever instantiated for client/backend flows.
//! - No BoringSSL handshake driven on the LB.
//! - No cert/key file loaded by this module.
//! - [`FlowEntry`] (the routing-table value) carries connection-ID +
//!   routing state ONLY — see the SAFETY/INVARIANT block on
//!   [`FlowEntry`] and the [`_flow_entry_field_audit`] destructuring
//!   audit at the bottom of this module.
//!
//! Verify gates: see `audit/quic/s15-design.md` §A2 and the task #6
//! description. Coverage target: ≥80% session-scope on this file.
//!
//! ## CF carry-forwards (S4 promote-report)
//!
//! - **CF-S15-PASSTHROUGH-FEATURE-GATING** — the `quic-terminate`
//!   cfg gate on the existing H3 termination tree (lib.rs +
//!   `terminate_loopback.rs`) is what closes the
//!   `cargo bloat --filter quiche` linkage proof.
//! - **CF-S15-FLOWENTRY-FIELD-AUDIT** — the
//!   [`_flow_entry_field_audit`] fn below. Any new `FlowEntry` field
//!   must be enumerated in the destructuring pattern AND
//!   type-witnessed; verifier code-reads on every change.
//! - **CF-S15-RETRY-NO-QUICHE** — hand-rolled [`build_retry_packet`]
//!   per RFC 9000 §17.2.5 + RFC 9001 §5.8.
//! - **CF-S15-DCID-MAP-XDP** — `UdpDataplane::dcid_map_fd` hook in
//!   `udp_dataplane.rs`; passthrough doesn't consume it in v1.0.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(any(test, feature = "test-gauges"))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use dashmap::DashMap;
use parking_lot::Mutex as PlMutex;
use ring::aead;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_balancer::{Backend, KeyedLoadBalancer, maglev::Maglev};
use lb_security::RetryTokenSigner;

use crate::public_header::{LongType, MAX_CID_LEN, PublicHeader, parse_public_header};
use crate::udp_dataplane::{
    MAX_UDP_DATAGRAM_SIZE, Packet, PacketHandler, TierPolicy, UdpDataplane, select_dataplane,
};

// ============================================================
// Constants
// ============================================================

/// Length of LB-chosen SCIDs (in Retry packets). RFC 9000 §17.2.5
/// requires `len > 0`; 16 bytes matches quiche / termination router.
const LB_SCID_LEN: usize = 16;

/// AEAD-AES-128-GCM tag length (RFC 9001 §5.8 fixed value).
const RETRY_INTEGRITY_TAG_LEN: usize = 16;

/// RFC 9001 §5.8 — fixed Retry Integrity Tag key for QUIC v1.
const RETRY_KEY_V1: [u8; 16] = [
    0xbe, 0x0c, 0x69, 0x0b, 0x9f, 0x66, 0x57, 0x5a, 0x1d, 0x76, 0x6b, 0x54, 0xe3, 0x68, 0xc8, 0x4e,
];

/// RFC 9001 §5.8 — fixed Retry Integrity Tag nonce for QUIC v1.
const RETRY_NONCE_V1: [u8; 12] = [
    0x46, 0x15, 0x99, 0xd3, 0x5d, 0x63, 0x2b, 0xf2, 0x23, 0x98, 0x25, 0xbb,
];

// ============================================================
// Parameters
// ============================================================

/// Construction parameters for [`PassthroughListener::spawn`].
#[derive(Debug, Clone)]
pub struct PassthroughParams {
    /// Bind address for the listener UDP socket.
    pub bind_addr: SocketAddr,
    /// Resolved backend addresses; consumed by Maglev consistent
    /// hashing on every Initial.
    pub backends: Vec<SocketAddr>,
    /// Path to a 32-byte retry-secret file. Generated with mode 0600
    /// if missing — same discipline as the termination listener.
    pub retry_secret_path: PathBuf,
    /// Maximum concurrent QUIC flows. Default 100_000 per owner
    /// ruling §9.4. Routing-table-entry cap = `2 * max`.
    pub max_quic_connections: usize,
    /// Minimum client-chosen DCID length accepted. Default 8 per
    /// owner ruling §9.3 (CVE 2022-30592-style cross-flow prefix
    /// collision defence).
    pub min_client_dcid_len: usize,
    /// Per-flow datagram backlog. Default 32; drop-newest on Full
    /// per design §5.1.
    pub per_flow_backlog: usize,
    /// Strict source-IP binding: when true, short-header packets
    /// whose source 4-tuple differs from the flow's recorded peer are
    /// dropped at the LB (breaks NAT-rebind path-migration but
    /// catches off-path spoofed-CID injection cheaply). Default
    /// **false** per owner ruling §9.1' — mobile availability is
    /// load-bearing for the default deployment.
    pub strict_source_binding: bool,
    /// Audit-log throttle window. 60s default — see §6.2.
    pub audit_throttle_window: Duration,
    /// Short-header DCID length to try first when no per-flow length
    /// is known. Default 20 (RFC 9000 §17.3 max) per §3.3 fallback.
    pub max_dcid_len_routed: usize,
    /// Whether the LB mints stateless Retry on no-token Initials
    /// (§6.5 Initial-flood defence per owner ruling §9.2). Default
    /// **true** for production deployments.
    ///
    /// **CF-S15-PASSTHROUGH-RETRY-ODCID:** with `mint_retry = true`,
    /// the second Initial's wire DCID is the LB-chosen new_scid and
    /// the backend cannot recover the client's original DCID
    /// (`original_destination_connection_id` transport param)
    /// without a side channel — RFC 9000 §17.2.5 anticipates this
    /// via the "Retry Service" pattern (token-embedded ODCID +
    /// backend extracts on verify); see CF for the S15.x / S16
    /// follow-up. When `mint_retry = false`, no-token Initials are
    /// forwarded to the backend verbatim and the backend's own
    /// `quiche::accept` either accepts directly or initiates its
    /// own Retry — the §6.5 Initial-flood defence is then the
    /// BACKEND's responsibility. Documented test/trusted-network
    /// escape; production leaves this `true`.
    pub mint_retry: bool,
    /// F-S20-2: idle-flow reaper threshold. A flow with no inbound packet
    /// for longer than this is reclaimed by the periodic idle sweep (its
    /// backend UDP socket fd + both pump tasks freed), bounding the table by
    /// the LIVE connection count rather than the LRU cap at
    /// `2 * max_quic_connections`. Default 60 s. `Duration::ZERO` disables
    /// the sweep (LRU-only — the pre-S21 behaviour).
    pub flow_idle_timeout: Duration,
    /// `quic_passthrough_*` observability handles (S15 A3). `None` when
    /// the listener is spawned without a metrics registry (unit tests,
    /// or a build that doesn't wire observability) — every event-site
    /// bump then becomes a no-op. Wired by
    /// `lb/main.rs::spawn_passthrough` off the shared `MetricsRegistry`.
    pub metrics: Option<lb_observability::PassthroughMetrics>,
}

impl PassthroughParams {
    /// Build params with reasonable defaults for the non-bind /
    /// non-backends knobs.
    #[must_use]
    pub fn new(
        bind_addr: SocketAddr,
        backends: Vec<SocketAddr>,
        retry_secret_path: PathBuf,
    ) -> Self {
        Self {
            bind_addr,
            backends,
            retry_secret_path,
            max_quic_connections: 100_000,
            min_client_dcid_len: 8,
            per_flow_backlog: 32,
            strict_source_binding: false,
            audit_throttle_window: Duration::from_secs(60),
            max_dcid_len_routed: MAX_CID_LEN,
            mint_retry: true,
            flow_idle_timeout: Duration::from_secs(60),
            metrics: None,
        }
    }
}

// ============================================================
// FlowEntry — the routing-table value, NO key material.
// ============================================================

// SAFETY/INVARIANT: FlowEntry holds no key material — passthrough
// never decrypts.
//
// Owner ruling §9.5 primary item 2: the LB cannot decrypt because
// the LB has no keys. This struct is the routing-state record kept
// per flow. Every field is non-cryptographic:
//
//   - backend          : SocketAddr (a destination address)
//   - short_dcid_len   : AtomicUsize (a wire-format length 0..=20)
//   - last_seen_ms     : AtomicU64 (millis-since-epoch for LRU)
//   - peer             : Mutex<SocketAddr> (the client's current
//                        4-tuple; updated on every recv for NAT
//                        rebind)
//   - backend_sock     : Arc<UdpSocket> (kernel-owned FD; no TLS
//                        wrapping)
//   - backlog_tx       : mpsc::Sender<Bytes> (datagram queue handle)
//
// **Adding a field**: enumerate it in [`_flow_entry_field_audit`]
// below AND assign it a type-witness `let _: &T = field_name;`.
// Compile error if the audit isn't updated. Per (b3)+(b4)+
// destructuring-audit ruling from the A2 plan-approval. Verifier
// code-reads on every change (Owner ruling §9.5: "explicit type-
// level assertion in passthrough.rs ... verifier-checked").
pub(crate) struct FlowEntry {
    /// The backend this flow is pinned to. Decided at first Initial
    /// (after Retry-token verify) via Maglev over the live backend
    /// set; immutable for the flow's lifetime (consistent-hash
    /// stability under backend-set churn).
    pub(crate) backend: SocketAddr,
    /// Short-header DCID length for this flow, recovered from the
    /// backend's first server-side long-header response (its SCID
    /// length byte). Default `0` before that response arrives.
    pub(crate) short_dcid_len: AtomicUsize,
    /// Last-seen millis-since-epoch for LRU eviction. Updated on
    /// every inbound packet that hits this entry.
    pub(crate) last_seen_ms: AtomicU64,
    /// Client's current 4-tuple (peer address). Updated on every
    /// inbound from the client; the reverse-task uses this to write
    /// backend→client. Supports NAT rebind path-migration: when the
    /// client's external port changes, this field tracks the new
    /// peer and the reverse-task starts writing there.
    pub(crate) peer: PlMutex<SocketAddr>,
    /// Per-flow backend UDP socket. `connect()`-ed to `backend` so
    /// the kernel filters incoming packets to those from the chosen
    /// backend (anti-spoof, design §3.4).
    pub(crate) backend_sock: Arc<UdpSocket>,
    /// Bounded mpsc queue feeding the per-flow forward task. Full →
    /// drop-newest (design §5.1).
    pub(crate) backlog_tx: mpsc::Sender<Vec<u8>>,
    /// F-S20-2: per-flow shutdown signal. Cancelled by [`reclaim_flows`]
    /// (LRU eviction or the idle sweep) so the reverse pump — which
    /// otherwise blocks indefinitely on `backend_sock.recv()` — exits and
    /// releases its `Arc<FlowEntry>`. Only then does the entry's strong
    /// count reach zero, dropping the backend UDP socket fd and closing the
    /// forward task's channel. Without this signal, removing the dispatch
    /// keys alone could not reclaim the fd/tasks for a flow whose backend is
    /// alive-but-silent (no recv error to break the loop). Not key material.
    pub(crate) closed: CancellationToken,
    /// LRU-eviction observed-flag (R13(b) gauge). Set by [`Drop`] when
    /// the entry's `Arc` reference count drops to zero (i.e. evicted
    /// from the dispatch table AND the per-flow forward task has
    /// exited). Tests poll this via [`PassthroughListener::dropped`].
    /// Behind `test-gauges` so production builds don't carry the
    /// atomic.
    #[cfg(any(test, feature = "test-gauges"))]
    pub(crate) dropped: Arc<AtomicBool>,
}

#[cfg(any(test, feature = "test-gauges"))]
impl Drop for FlowEntry {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[allow(dead_code)]
impl FlowEntry {
    /// Update `last_seen_ms` to now.
    pub(crate) fn touch(&self, now: Instant, epoch: Instant) {
        let elapsed_ms =
            u64::try_from(now.saturating_duration_since(epoch).as_millis()).unwrap_or(u64::MAX);
        self.last_seen_ms.store(elapsed_ms, Ordering::Relaxed);
    }

    /// Set the current peer (NAT rebind handler).
    pub(crate) fn set_peer(&self, peer: SocketAddr) {
        *self.peer.lock() = peer;
    }

    /// Current peer 4-tuple.
    pub(crate) fn get_peer(&self) -> SocketAddr {
        *self.peer.lock()
    }
}

/// CF-S15-FLOWENTRY-FIELD-AUDIT — destructuring audit. Any new
/// `FlowEntry` field forces the compiler to surface this pattern as
/// incomplete, which forces the author to add it AND assign a type
/// witness. The witness asserts the field's STATIC type is not a
/// key-shaped type from `ring`/`boring`/`quiche`.
///
/// Verifier code-reads on every change. Owner ruling §9.5 primary
/// item 2: "FlowEntry MUST hold no key material — passthrough never
/// decrypts."
#[cfg(any(test, debug_assertions))]
#[allow(dead_code)]
fn _flow_entry_field_audit(e: &FlowEntry) {
    let FlowEntry {
        backend,
        short_dcid_len,
        last_seen_ms,
        peer,
        backend_sock,
        backlog_tx,
        closed,
        #[cfg(any(test, feature = "test-gauges"))]
        dropped,
    } = e;
    // Type-witnesses: each field's static type is enumerated here.
    // ANY change to FlowEntry → compile error here.
    let _: &SocketAddr = backend;
    let _: &AtomicUsize = short_dcid_len;
    let _: &AtomicU64 = last_seen_ms;
    let _: &PlMutex<SocketAddr> = peer;
    let _: &Arc<UdpSocket> = backend_sock;
    let _: &mpsc::Sender<Vec<u8>> = backlog_tx;
    let _: &CancellationToken = closed;
    #[cfg(any(test, feature = "test-gauges"))]
    let _: &Arc<AtomicBool> = dropped;
    // None of the above types are key material (AEAD::Key,
    // ring::aead::*Key, boring::Aead, quiche::Connection, ...).
}

// ============================================================
// Hand-rolled Retry packet writer (CF-S15-RETRY-NO-QUICHE)
// ============================================================

/// CF-S15-DCID-HASH: SipHash-style multiply-shift over the client DCID
/// to feed `Maglev::pick_with_key`. We don't need a cryptographic hash
/// (Maglev's permutation is the consistency layer); a fast non-zero
/// mixing function is enough. Deterministic across runs of the same
/// binary (same seed).
fn hash_dcid_for_maglev(dcid: &[u8]) -> u64 {
    // FxHash-shaped multiply-add. Same finalizer as
    // lb_balancer::maglev::hash_str so the distribution behaves
    // identically to the L7-affinity path.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV offset basis
    for &b in dcid {
        h = h
            .wrapping_mul(0x517c_c1b7_2722_0a95)
            .wrapping_add(u64::from(b));
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// Build a QUIC v1 Retry packet per RFC 9000 §17.2.5 and compute its
/// 16-byte AEAD-AES-128-GCM Retry Integrity Tag per RFC 9001 §5.8.
///
/// The output layout (on-wire bytes) is:
///   `byte0 | version | DCID_len | DCID | SCID_len | SCID | token | integrity_tag(16)`
///
/// The Retry Pseudo-Packet (RFC 9001 §5.8) is constructed as:
///   `ODCID_len(1) | ODCID | byte0 | version | DCID_len | DCID | SCID_len | SCID | token`
///
/// and AAD-only sealed under the fixed v1 Retry key + nonce.
///
/// # Arguments
///
/// * `odcid` — Original Destination Connection ID (the DCID the
///   client used in its first Initial; goes into the pseudo-packet
///   ONLY, not the wire bytes).
/// * `client_scid` — Client's first-Initial SCID, becomes the on-wire
///   DCID (per RFC 9000 §17.2.5).
/// * `new_scid` — LB-chosen 16-byte SCID; becomes the routing DCID
///   for the client's second Initial.
/// * `version` — Wire version field; echo client's first-Initial
///   version.
/// * `token` — Retry token bytes (signed by `RetryTokenSigner`).
/// * `out` — Output buffer; cleared and written into.
///
/// # Errors
///
/// Returns `Err` if `ring::aead` rejects the inputs (sealing an
/// empty plaintext under AES-128-GCM is infallible in practice; this
/// covers ring's API surface).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_retry_packet(
    odcid: &[u8],
    client_scid: &[u8],
    new_scid: &[u8; LB_SCID_LEN],
    version: u32,
    token: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), String> {
    if odcid.len() > MAX_CID_LEN {
        return Err(format!("ODCID len {} > MAX_CID_LEN", odcid.len()));
    }
    if client_scid.len() > MAX_CID_LEN {
        return Err(format!(
            "client SCID len {} > MAX_CID_LEN",
            client_scid.len()
        ));
    }

    // RFC 9000 §17.2 byte0 for Retry:
    //   bit7 = 1 (Long header)
    //   bit6 = 1 (Fixed Bit)
    //   bits5-4 = 0b11 (Type=Retry)
    //   bits3-0 = unused on wire (zero is fine; HP doesn't apply to
    //             Retry — it has no PN).
    let byte0 = 0b1111_0000u8;

    // Build the Retry Pseudo-Packet for AAD.
    let mut pseudo = BytesMut::with_capacity(
        1 + odcid.len() + 1 + 4 + 1 + client_scid.len() + 1 + LB_SCID_LEN + token.len(),
    );
    pseudo.extend_from_slice(&[u8::try_from(odcid.len()).unwrap_or(0)]);
    pseudo.extend_from_slice(odcid);
    pseudo.extend_from_slice(&[byte0]);
    pseudo.extend_from_slice(&version.to_be_bytes());
    pseudo.extend_from_slice(&[u8::try_from(client_scid.len()).unwrap_or(0)]);
    pseudo.extend_from_slice(client_scid);
    pseudo.extend_from_slice(&[u8::try_from(LB_SCID_LEN).unwrap_or(0)]);
    pseudo.extend_from_slice(new_scid);
    pseudo.extend_from_slice(token);

    // Compute integrity tag via AEAD-AES-128-GCM aad-only seal.
    let unbound = aead::UnboundKey::new(&aead::AES_128_GCM, &RETRY_KEY_V1)
        .map_err(|e| format!("ring UnboundKey: {e}"))?;
    let key = aead::LessSafeKey::new(unbound);
    let nonce = aead::Nonce::assume_unique_for_key(RETRY_NONCE_V1);
    let aad = aead::Aad::from(pseudo.as_ref());

    // ring requires a mutable in-place buffer even for AAD-only.
    let mut empty: [u8; 0] = [];
    let tag = key
        .seal_in_place_separate_tag(nonce, aad, &mut empty)
        .map_err(|e| format!("ring seal: {e}"))?;
    let tag_bytes = tag.as_ref();
    if tag_bytes.len() != RETRY_INTEGRITY_TAG_LEN {
        return Err(format!(
            "unexpected tag length {} != {}",
            tag_bytes.len(),
            RETRY_INTEGRITY_TAG_LEN
        ));
    }

    // Emit on-wire bytes.
    out.clear();
    out.reserve(1 + 4 + 1 + client_scid.len() + 1 + LB_SCID_LEN + token.len() + tag_bytes.len());
    out.push(byte0);
    out.extend_from_slice(&version.to_be_bytes());
    out.push(u8::try_from(client_scid.len()).unwrap_or(0));
    out.extend_from_slice(client_scid);
    out.push(u8::try_from(LB_SCID_LEN).unwrap_or(0));
    out.extend_from_slice(new_scid);
    out.extend_from_slice(token);
    out.extend_from_slice(tag_bytes);
    Ok(())
}

/// Test-only re-export of [`build_retry_packet`] for integration tests
/// (verify gate A2-2: 1000-case byte-equality differential vs
/// `quiche::retry`).
///
/// CF-S15-TESTGAUGES-EXPORT-NARROW: previously gated behind
/// `#[cfg(any(test, feature = "test-gauges"))]`, but `cfg(test)` is
/// false during downstream integration-test compile (only true on the
/// crate's OWN tests), so the gate forced every consumer to enable
/// `test-gauges` even when only this one symbol was needed. Owner
/// ruling §9.5 "construction over observation" doctrine for verify-
/// gate artefacts: keep `build_retry_packet` private; expose this
/// underscore-prefixed `#[doc(hidden)]` wrapper unconditionally so
/// the cfg shape matches its consumer pattern. Production-surface
/// impact: zero (still doc-hidden, underscore-prefixed, doc-typed
/// "Test-only").
#[doc(hidden)]
pub fn _test_build_retry_packet(
    odcid: &[u8],
    client_scid: &[u8],
    new_scid: &[u8; 16],
    version: u32,
    token: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), String> {
    build_retry_packet(odcid, client_scid, new_scid, version, token, out)
}

// ============================================================
// Shared router context — held inside an Arc by spawn() and cloned
// into the per-packet callback closures and per-flow forward/reverse
// tasks.
// ============================================================

/// Routing-table entries: one per known DCID. A flow has up to 2 keys
/// (client-chosen DCID at Initial-time; LB-chosen SCID after Retry).
type FlowTable = DashMap<Vec<u8>, Arc<FlowEntry>>;

struct RouterCtx {
    params: PassthroughParams,
    /// Resolved Maglev table over the backend set. Held in an Arc so
    /// future backend-set reloads can hot-swap; v1 is static post-spawn.
    maglev: Maglev,
    /// `Backend` view of `params.backends` matching the index order in
    /// the Maglev table. Built once at spawn.
    backends: Vec<Backend>,
    retry_signer: Arc<RetryTokenSigner>,
    table: Arc<FlowTable>,
    /// Listener-side UDP socket (write half of the recv loop), used to
    /// send Retry packets back to clients and reverse-direction
    /// backend→client traffic.
    listener_sock: Arc<dyn UdpDataplane>,
    /// Process-relative monotonic epoch for `last_seen_ms` (kept small
    /// vs absolute timestamps).
    epoch: Instant,
    /// Audit-log throttle state (S15 A3, design §A3). One slot per audit
    /// category holding the epoch-relative millis at which that
    /// category last emitted a `warn!` audit line. Initialised to
    /// [`AUDIT_NEVER`] so the FIRST event in every category always
    /// emits; thereafter [`audit_allow`] gates to one line per
    /// `params.audit_throttle_window`. The verifier's saturation test
    /// asserts ONE line per window, not one per event.
    audit_last_source_binding_ms: AtomicU64,
    audit_last_cap_hit_ms: AtomicU64,
}

/// Sentinel for "no audit line emitted yet" — chosen so the first event
/// (any `now_ms`) clears the window check in [`audit_allow`]. Distinct
/// from a real `0` epoch-millis reading.
const AUDIT_NEVER: u64 = u64::MAX;

/// Audit-throttle gate. Returns `true` (and records `now_ms`) if an
/// audit line for the category backed by `last_emit` may be emitted now —
/// i.e. the slot is [`AUDIT_NEVER`] (first event) or `now_ms` is at least
/// `window` past the last emit. Returns `false` otherwise. Uses
/// `fetch_update` so two threads racing the same window emit exactly once.
fn audit_allow(last_emit: &AtomicU64, now_ms: u64, window: Duration) -> bool {
    let window_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
    last_emit
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |prev| {
            if prev == AUDIT_NEVER || now_ms.saturating_sub(prev) >= window_ms {
                Some(now_ms)
            } else {
                None
            }
        })
        .is_ok()
}

/// Hash a DCID into a Maglev pick.
fn pick_backend(ctx: &RouterCtx, dcid: &[u8]) -> Option<SocketAddr> {
    let key = hash_dcid_for_maglev(dcid);
    let idx = ctx.maglev.pick_with_key(&ctx.backends, key).ok()?;
    ctx.params.backends.get(idx).copied()
}

/// Set `quic_passthrough_flows` to the current dispatch-table size.
/// Called after each table insert/eviction. The gauge counts
/// dispatch-table entries (≈ flows; a migrated flow may briefly hold 2
/// CID keys) per owner ruling Q2. Reading `table.len()` after the
/// mutation makes the gauge self-correct: under concurrent dispatch the
/// last writer's reading wins and converges to the live count.
fn set_flows_gauge(ctx: &RouterCtx) {
    if let Some(m) = &ctx.params.metrics {
        m.flows
            .set(i64::try_from(ctx.table.len()).unwrap_or(i64::MAX));
    }
}

/// LRU eviction at cap. Walks the flow table and drops the entry with
/// the oldest `last_seen_ms`. v1 is single-evict per cap-hit; bulk
/// eviction lives in A3. Returns the number of dispatch-table entries
/// removed (typically 1 or 2 — flow may have two keys).
fn evict_oldest(ctx: &RouterCtx) -> usize {
    let mut oldest_last = u64::MAX;
    let mut victim: Option<Arc<FlowEntry>> = None;
    for entry in ctx.table.iter() {
        let last = entry.value().last_seen_ms.load(Ordering::Relaxed);
        if victim.is_none() || last < oldest_last {
            oldest_last = last;
            victim = Some(Arc::clone(entry.value()));
        }
    }
    match victim {
        Some(v) => reclaim_flows(ctx, std::slice::from_ref(&v)),
        None => 0,
    }
}

/// F-S20-2 — reclaim a set of flows. SINGLE-SOURCED reclamation (R12) for
/// both LRU eviction ([`evict_oldest`]) and the periodic idle sweep
/// ([`sweep_idle_flows`]). For each victim:
///
/// 1. **Cancel its `closed` token** so the per-flow reverse pump exits its
///    otherwise-indefinite blocking `backend_sock.recv()` and releases its
///    `Arc<FlowEntry>`. This is the load-bearing step — without it, removing
///    the dispatch keys alone cannot reclaim a flow whose backend is
///    alive-but-silent (no recv error to break the loop), so the fd + tasks
///    would leak (the F-S20-2 mechanism).
/// 2. **Remove every dispatch-table key** pointing at the victim (Arc
///    identity; a migrated flow may hold 2 keys). Borrow-and-collect to
///    avoid an iterator/remove deadlock in DashMap.
///
/// Once the reverse task exits and the keys are gone the entry's strong
/// count reaches zero: its `Drop` closes the backend UDP socket fd and the
/// forward task's channel (or the forward task's own `closed` select fires).
/// Bumps `flows_evicted_total` ONCE per flow actually reclaimed (owner ruling
/// Q1 — not per removed CID key, which would double-count 2-key flows).
/// Returns the number of dispatch-table entries removed (so the LRU caller
/// can detect "nothing evicted" and avoid spinning).
fn reclaim_flows(ctx: &RouterCtx, victims: &[Arc<FlowEntry>]) -> usize {
    if victims.is_empty() {
        return 0;
    }
    // Signal each victim's pumps to stop (idempotent; `CancellationToken`).
    for v in victims {
        v.closed.cancel();
    }
    let mut per_victim_removed = vec![0usize; victims.len()];
    let keys: Vec<(usize, Vec<u8>)> = ctx
        .table
        .iter()
        .filter_map(|kv| {
            victims
                .iter()
                .position(|v| Arc::ptr_eq(kv.value(), v))
                .map(|i| (i, kv.key().clone()))
        })
        .collect();
    let mut removed = 0usize;
    for (i, k) in keys {
        if ctx.table.remove(&k).is_some() {
            removed += 1;
            if let Some(c) = per_victim_removed.get_mut(i) {
                *c += 1;
            }
        }
    }
    let flows_reclaimed = per_victim_removed.iter().filter(|&&c| c > 0).count();
    if flows_reclaimed > 0 {
        if let Some(m) = &ctx.params.metrics {
            m.flows_evicted_total
                .inc_by(u64::try_from(flows_reclaimed).unwrap_or(u64::MAX));
        }
        // Self-correcting gauge: re-read the post-removal table size.
        set_flows_gauge(ctx);
    }
    removed
}

/// F-S20-2 — periodic idle-flow reaper. Reclaims every flow whose
/// `last_seen_ms` is older than `idle_ms`, bounding the table by the LIVE
/// connection count instead of the LRU cap at `2 * max_quic_connections`.
/// Passthrough cannot observe the encrypted CONNECTION_CLOSE, so a flow for
/// a closed connection would otherwise persist (pinning a backend UDP socket
/// fd + 2 pump tasks) until the cap — the S20 leak. Returns the number of
/// flows reclaimed this sweep.
fn sweep_idle_flows(ctx: &RouterCtx, idle_ms: u64) -> usize {
    let now_ms = elapsed_ms(Instant::now(), ctx.epoch);
    let mut victims: Vec<Arc<FlowEntry>> = Vec::new();
    for entry in ctx.table.iter() {
        let last = entry.value().last_seen_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= idle_ms
            && !victims.iter().any(|v| Arc::ptr_eq(v, entry.value()))
        {
            victims.push(Arc::clone(entry.value()));
        }
    }
    let n = victims.len();
    reclaim_flows(ctx, &victims);
    n
}

/// Handle one inbound datagram from the client.
async fn handle_inbound(ctx: Arc<RouterCtx>, data: Vec<u8>, from: SocketAddr) {
    let parsed = match parse_public_header(&data, default_short_dcid_len(&ctx)) {
        Ok(h) => h,
        Err(e) => {
            if let Some(m) = &ctx.params.metrics {
                m.header_parse_errors_total.inc();
            }
            tracing::trace!(error = %e, peer = %from, "header parse error");
            return;
        }
    };

    match parsed {
        PublicHeader::Long {
            ty,
            version,
            dcid,
            scid,
            token,
            ..
        } => match ty {
            LongType::Initial => {
                handle_initial(ctx, data.clone(), from, version, dcid, scid, token).await;
            }
            LongType::ZeroRtt | LongType::Handshake => {
                // Either retransmit of an Initial whose handshake is
                // mid-flight, or genuine post-handshake long packet.
                // Either way: look up by DCID; if present, forward; if
                // not, drop (no token, can't mint Retry off this).
                forward_long_existing(&ctx, &data, dcid).await;
            }
            LongType::Retry | LongType::VersionNegotiation => {
                // Client-origin Retry / VN are not legal on the LB-
                // facing leg. Drop.
                tracing::trace!(peer = %from, ?ty, "dropped client-origin Retry/VN");
            }
        },
        PublicHeader::Short { dcid } => {
            // The parser returned a Short with the default-length DCID;
            // try multi-length fallback if the default missed.
            forward_short(&ctx, &data, dcid, from).await;
        }
    }
}

/// Pick the default short-header DCID length. v1 uses
/// `params.max_dcid_len_routed` as the single-len fast-path.
fn default_short_dcid_len(ctx: &RouterCtx) -> usize {
    ctx.params.max_dcid_len_routed
}

/// Initial-packet handler. §3.2a in the design.
async fn handle_initial(
    ctx: Arc<RouterCtx>,
    pkt: Vec<u8>,
    from: SocketAddr,
    version: u32,
    dcid: &[u8],
    scid: &[u8],
    token: Option<&[u8]>,
) {
    // Cap-violation defence: drop initials with DCIDs shorter than the
    // floor (§6.8).
    if dcid.len() < ctx.params.min_client_dcid_len {
        tracing::debug!(
            peer = %from,
            dcid_len = dcid.len(),
            floor = ctx.params.min_client_dcid_len,
            "drop: dcid below floor"
        );
        return;
    }

    // Retransmit? Look up Table[dcid].
    if let Some(entry) = ctx.table.get(dcid) {
        let flow = Arc::clone(entry.value());
        drop(entry);
        flow.touch(Instant::now(), ctx.epoch);
        flow.set_peer(from);
        let _ = flow.backlog_tx.try_send(pkt);
        return;
    }

    // New connection.
    //
    // CF-S15-PASSTHROUGH-RETRY-ODCID: the §6.5 Initial-flood defence
    // mints a Retry on no-token Initials, but the LB-chosen new_scid
    // in that Retry then becomes the second-Initial wire DCID — the
    // backend cannot recover the client's ORIGINAL DCID without a
    // side channel, so a real-quiche backend rejects the resulting
    // `original_destination_connection_id` transport param. RFC 9000
    // §17.2.5 anticipates this via the "Retry Service" pattern
    // (token-embedded ODCID + backend extracts on verify); deferred
    // to S15.x / S16.
    //
    // The `mint_retry` knob (default `true`) is the production-vs-
    // dev/trusted-network escape: when `false`, no-token Initials
    // are forwarded verbatim and the BACKEND handles Initial-flood
    // defence (either it `accept`s directly or initiates its own
    // backend-side Retry, which the LB just forwards).
    let tok = token.unwrap_or(&[]);
    if tok.is_empty() && ctx.params.mint_retry {
        // Mint Retry (no state allocation; just send and forget).
        let new_scid = sample_lb_scid();
        let retry_token = ctx.retry_signer.mint(from, dcid);
        let mut out = Vec::with_capacity(128);
        if let Err(e) = build_retry_packet(dcid, scid, &new_scid, version, &retry_token, &mut out) {
            tracing::debug!(error = %e, peer = %from, "build_retry_packet");
            return;
        }
        if let Err(e) = ctx.listener_sock.send_to(&out, from).await {
            tracing::debug!(error = %e, peer = %from, "send Retry");
        } else if let Some(m) = &ctx.params.metrics {
            // Count only Retries actually put on the wire.
            m.retry_minted_total.inc();
        }
        return;
    }

    // Token present → verify. When mint_retry=false AND no token,
    // we skip verify entirely (the BACKEND is the §6.5 defender).
    let now = Instant::now();
    if !tok.is_empty() {
        if let Err(e) = ctx.retry_signer.verify(tok, from, now) {
            if let Some(m) = &ctx.params.metrics {
                m.retry_rejected_total.inc();
            }
            tracing::trace!(error = %e, peer = %from, "retry token verify failed");
            return;
        }
    }

    // Cap-check; evict oldest on hit (§5 LRU). Cap is on UNIQUE flows;
    // dispatch table has up to 2 keys per flow, so the table-size
    // bound is 2× cap.
    let cap = ctx.params.max_quic_connections;
    if ctx.table.len() >= cap.saturating_mul(2) {
        // Cap-hit audit line (design §A3, throttled one-per-window).
        let now_ms = elapsed_ms(now, ctx.epoch);
        if audit_allow(
            &ctx.audit_last_cap_hit_ms,
            now_ms,
            ctx.params.audit_throttle_window,
        ) {
            tracing::warn!(
                event = "audit/quic_passthrough_cap_hit",
                peer = %from,
                table_len = ctx.table.len(),
                cap,
                "passthrough flow cap hit; evicting oldest flow(s)"
            );
        }
        while ctx.table.len() >= cap.saturating_mul(2) {
            if evict_oldest(&ctx) == 0 {
                break; // no entries to evict (table is empty); avoid spin.
            }
        }
    }

    // Maglev pick.
    let Some(backend) = pick_backend(&ctx, dcid) else {
        tracing::debug!(peer = %from, "no backend available");
        return;
    };

    // Open per-flow backend UDP socket.
    let bind_any: SocketAddr = match backend {
        SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap_or(backend),
        SocketAddr::V6(_) => "[::]:0".parse().unwrap_or(backend),
    };
    let backend_sock = match UdpSocket::bind(bind_any).await {
        Ok(s) => s,
        Err(e) => {
            if let Some(m) = &ctx.params.metrics {
                m.backend_socket_errors_total.inc();
            }
            tracing::debug!(error = %e, "bind backend socket");
            return;
        }
    };
    if let Err(e) = backend_sock.connect(backend).await {
        if let Some(m) = &ctx.params.metrics {
            m.backend_socket_errors_total.inc();
        }
        tracing::debug!(error = %e, %backend, "connect backend socket");
        return;
    }
    let backend_sock = Arc::new(backend_sock);

    let (backlog_tx, backlog_rx) = mpsc::channel::<Vec<u8>>(ctx.params.per_flow_backlog);

    #[cfg(any(test, feature = "test-gauges"))]
    let dropped = Arc::new(AtomicBool::new(false));

    let flow = Arc::new(FlowEntry {
        backend,
        short_dcid_len: AtomicUsize::new(0),
        last_seen_ms: AtomicU64::new(elapsed_ms(now, ctx.epoch)),
        peer: PlMutex::new(from),
        backend_sock: Arc::clone(&backend_sock),
        backlog_tx: backlog_tx.clone(),
        closed: CancellationToken::new(),
        #[cfg(any(test, feature = "test-gauges"))]
        dropped: Arc::clone(&dropped),
    });

    // Register routing key for the client-chosen DCID. The LB-chosen
    // new_scid is NOT in the wire packet on this branch (this branch
    // is the second Initial, where the client has already received our
    // Retry and is now sending DCID = our new_scid). Per §3.6 the
    // routing DCID *is* our LB-chosen SCID, but it's already present
    // as the wire DCID — so the existing key insertion does the right
    // thing.
    ctx.table.insert(dcid.to_vec(), Arc::clone(&flow));
    // Self-correcting gauge: re-read the post-insert table size.
    set_flows_gauge(&ctx);

    // Forward the inbound packet first.
    let _ = backlog_tx.try_send(pkt);

    // Spawn per-flow forward pump (client→backend). Exits when the backlog
    // channel closes (all `backlog_tx` senders dropped — i.e. the FlowEntry
    // dropped) OR the flow's `closed` token fires (F-S20-2 reclaim), so a
    // reaped flow's forward task tears down promptly rather than lingering.
    let backend_sock_fwd = Arc::clone(&backend_sock);
    let ctx_fwd = Arc::clone(&ctx);
    let closed_fwd = flow.closed.clone();
    tokio::spawn(async move {
        let mut rx = backlog_rx;
        loop {
            let buf = tokio::select! {
                biased;
                () = closed_fwd.cancelled() => break,
                maybe = rx.recv() => match maybe {
                    Some(buf) => buf,
                    None => break,
                },
            };
            if let Err(e) = backend_sock_fwd.send(&buf).await {
                if let Some(m) = &ctx_fwd.params.metrics {
                    m.backend_socket_errors_total.inc();
                }
                tracing::trace!(error = %e, "forward send failed");
                break;
            }
        }
    });

    // Spawn per-flow reverse pump (backend→client).
    let ctx_rev = Arc::clone(&ctx);
    let flow_rev = Arc::clone(&flow);
    tokio::spawn(async move { reverse_pump(ctx_rev, flow_rev).await });
}

/// Reverse-direction pump for one flow.
async fn reverse_pump(ctx: Arc<RouterCtx>, flow: Arc<FlowEntry>) {
    let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
    loop {
        // F-S20-2: race the blocking backend recv against the per-flow
        // `closed` signal so an LRU-evicted / idle-reaped flow exits here,
        // releasing this task's `Arc<FlowEntry>` (→ fd + forward task freed).
        // Without this, a flow whose backend is alive-but-silent would block
        // on `recv()` forever and never be reclaimed.
        let n = tokio::select! {
            biased;
            () = flow.closed.cancelled() => break,
            r = flow.backend_sock.recv(&mut buf) => match r {
                Ok(n) => n,
                Err(e) => {
                    if let Some(m) = &ctx.params.metrics {
                        m.backend_socket_errors_total.inc();
                    }
                    tracing::trace!(error = %e, "backend recv");
                    break;
                }
            },
        };
        let slice = buf.get(..n).unwrap_or(&[]);

        // Peek the long-header server-side SCID to discover the flow's
        // routing DCID (the server's SCID becomes the client's next
        // DCID; we register it so subsequent client packets routed by
        // that DCID land on this flow). Short-header reverse packets
        // have no CIDs the LB can see — pass through.
        if let Ok(PublicHeader::Long { scid, .. }) = parse_public_header(slice, 0) {
            if !scid.is_empty() {
                let key = scid.to_vec();
                // Avoid clobbering an existing entry (e.g. a different
                // flow already keyed by this SCID, which would be a
                // backend collision; in practice we trust the backend's
                // SCID is unique).
                ctx.table.entry(key).or_insert_with(|| Arc::clone(&flow));
                flow.short_dcid_len.store(scid.len(), Ordering::Relaxed);
            }
        }

        let peer = flow.get_peer();
        if let Err(e) = ctx.listener_sock.send_to(slice, peer).await {
            tracing::trace!(error = %e, "reverse send failed");
            // Don't break on transient send errors — UDP is best-
            // effort and the next packet may go through.
        }
    }
}

/// Forward an existing flow's long-header (non-Initial) packet by DCID.
async fn forward_long_existing(ctx: &RouterCtx, pkt: &[u8], dcid: &[u8]) {
    if let Some(entry) = ctx.table.get(dcid) {
        let flow = Arc::clone(entry.value());
        drop(entry);
        flow.touch(Instant::now(), ctx.epoch);
        let _ = flow.backlog_tx.try_send(pkt.to_vec());
    }
}

/// Short-header inbound: try single-len fast path, then walk the set of
/// known short-DCID lengths.
async fn forward_short(ctx: &RouterCtx, pkt: &[u8], default_dcid: &[u8], from: SocketAddr) {
    // Fast path: default-length DCID (already parsed for us).
    if let Some(entry) = ctx.table.get(default_dcid) {
        let flow = Arc::clone(entry.value());
        drop(entry);
        if !forward_short_via(ctx, &flow, pkt, from) {
            return; // strict-source-binding drop
        }
        flow.touch(Instant::now(), ctx.epoch);
        flow.set_peer(from);
        let _ = flow.backlog_tx.try_send(pkt.to_vec());
        return;
    }

    // Multi-length fallback: collect distinct known short_dcid_lens.
    let mut lens: Vec<usize> = ctx
        .table
        .iter()
        .map(|kv| kv.value().short_dcid_len.load(Ordering::Relaxed))
        .filter(|&l| l > 0 && l <= MAX_CID_LEN && l != default_dcid.len())
        .collect();
    lens.sort_unstable();
    lens.dedup();
    for len in lens {
        let end = 1usize.saturating_add(len);
        let Some(dcid) = pkt.get(1..end) else {
            continue;
        };
        if let Some(entry) = ctx.table.get(dcid) {
            let flow = Arc::clone(entry.value());
            drop(entry);
            if !forward_short_via(ctx, &flow, pkt, from) {
                return;
            }
            flow.touch(Instant::now(), ctx.epoch);
            flow.set_peer(from);
            let _ = flow.backlog_tx.try_send(pkt.to_vec());
            return;
        }
    }
    // Miss → drop. §3.3 documents this as expected; client retransmit
    // will hit when a long header refreshes the dispatch.
}

/// Strict source-binding gate (§6.3, owner ruling §9.1'). Returns
/// `true` if the packet should be forwarded; `false` to drop.
fn forward_short_via(ctx: &RouterCtx, flow: &FlowEntry, _pkt: &[u8], from: SocketAddr) -> bool {
    if !ctx.params.strict_source_binding {
        return true;
    }
    let recorded = flow.get_peer();
    if recorded != from {
        // Per-event debug trace (kept for full visibility).
        tracing::trace!(
            recorded = %recorded,
            observed = %from,
            "strict_source_binding drop"
        );
        // Throttled audit record (design §A3): one `warn!` per
        // `audit_throttle_window`, NOT one per dropped packet — a
        // spoofing flood must not flood the audit log.
        let now_ms = elapsed_ms(Instant::now(), ctx.epoch);
        if audit_allow(
            &ctx.audit_last_source_binding_ms,
            now_ms,
            ctx.params.audit_throttle_window,
        ) {
            tracing::warn!(
                event = "audit/source_binding_violation",
                recorded = %recorded,
                observed = %from,
                "strict source-binding violation; dropping short-header packet from unexpected 4-tuple"
            );
        }
        return false;
    }
    true
}

fn sample_lb_scid() -> [u8; LB_SCID_LEN] {
    let mut scid = [0u8; LB_SCID_LEN];
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut scid).is_err() {
        // RNG failure on a supported platform is effectively impossible;
        // fall back to a process-unique counter rather than panicking.
        use std::sync::atomic::AtomicU64;
        static FALLBACK: AtomicU64 = AtomicU64::new(0);
        let n = FALLBACK.fetch_add(1, Ordering::Relaxed);
        scid[..8].copy_from_slice(&n.to_be_bytes());
    }
    scid
}

fn elapsed_ms(now: Instant, epoch: Instant) -> u64 {
    u64::try_from(now.saturating_duration_since(epoch).as_millis()).unwrap_or(u64::MAX)
}

// ============================================================
// PassthroughListener
// ============================================================

/// A running Mode A passthrough listener.
pub struct PassthroughListener {
    local_addr: SocketAddr,
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
    /// Held so the signer survives at least as long as the listener.
    _retry_signer: Arc<RetryTokenSigner>,
    /// Test-only handle to the flow table for verify gates.
    #[cfg(any(test, feature = "test-gauges"))]
    table: Arc<FlowTable>,
}

impl PassthroughListener {
    /// Bind a UDP socket, load (or generate) the retry secret, build
    /// the Maglev table over the backend set, and spawn the recv loop.
    ///
    /// # Errors
    ///
    /// Returns the OS bind error or a backend-set rejection
    /// (empty-backends → `InvalidInput`).
    pub async fn spawn(
        params: PassthroughParams,
        shutdown: CancellationToken,
    ) -> std::io::Result<Self> {
        if params.backends.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "passthrough requires at least one backend",
            ));
        }

        let dataplane = select_dataplane(params.bind_addr, TierPolicy::Auto)
            .await
            .map_err(|e| std::io::Error::other(format!("dataplane bind: {e}")))?;
        let local_addr = dataplane.local_addr();
        let retry_signer = Arc::new(load_or_generate_retry_secret(&params.retry_secret_path)?);
        let table: Arc<FlowTable> = Arc::new(DashMap::new());

        // Build the Backend view for Maglev.
        let backends: Vec<Backend> = params
            .backends
            .iter()
            .enumerate()
            .map(|(i, sa)| Backend {
                id: format!("backend-{i}-{sa}"),
                weight: 1,
                active_connections: 0,
                active_requests: 0,
                latency_ewma_ns: 0,
                state: None,
            })
            .collect();
        let maglev =
            Maglev::new(&backends).map_err(|e| std::io::Error::other(format!("maglev: {e}")))?;

        let ctx = Arc::new(RouterCtx {
            params,
            maglev,
            backends,
            retry_signer: Arc::clone(&retry_signer),
            table: Arc::clone(&table),
            listener_sock: Arc::clone(&dataplane),
            epoch: Instant::now(),
            audit_last_source_binding_ms: AtomicU64::new(AUDIT_NEVER),
            audit_last_cap_hit_ms: AtomicU64::new(AUDIT_NEVER),
        });

        tracing::info!(
            address = %local_addr,
            protocol = "quic-passthrough",
            backends = ctx.params.backends.len(),
            "QUIC passthrough listener bound"
        );

        // F-S20-2: periodic idle-flow reaper. Bounds the flow table by the
        // LIVE connection count (a flow idle past `flow_idle_timeout` is
        // reclaimed — backend UDP socket fd + both pump tasks freed) rather
        // than waiting for the LRU cap at `2 * max_quic_connections`.
        // `Duration::ZERO` disables it (LRU-only, pre-S21 behaviour).
        let idle = ctx.params.flow_idle_timeout;
        if !idle.is_zero() {
            let sweep_ctx = Arc::clone(&ctx);
            let sweep_shutdown = shutdown.clone();
            let idle_ms = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
            // Sweep cadence = a quarter of the idle window, clamped to
            // [1s, 10s], so a reaped flow is freed within ~1.25× the idle
            // timeout without busy-spinning on a tiny table.
            let period = Duration::from_millis((idle_ms / 4).max(1))
                .clamp(Duration::from_secs(1), Duration::from_secs(10));
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(period);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        biased;
                        () = sweep_shutdown.cancelled() => break,
                        _ = tick.tick() => {
                            let reaped = sweep_idle_flows(&sweep_ctx, idle_ms);
                            if reaped > 0 {
                                tracing::debug!(
                                    reaped,
                                    idle_ms,
                                    "passthrough idle-flow sweep reclaimed flows"
                                );
                            }
                        }
                    }
                }
            });
        }

        let shutdown_for_loop = shutdown.clone();
        let dataplane_for_loop = Arc::clone(&dataplane);
        let handle = tokio::spawn(async move {
            let ctx_cb = Arc::clone(&ctx);
            let on_packet: PacketHandler<'_> = Arc::new(move |pkt: Packet<'_>| {
                let ctx_inner = Arc::clone(&ctx_cb);
                let data = pkt.data.to_vec();
                let from = pkt.from;
                Box::pin(async move {
                    handle_inbound(ctx_inner, data, from).await;
                })
            });
            if let Err(e) = dataplane_for_loop
                .recv_loop(shutdown_for_loop, on_packet)
                .await
            {
                tracing::warn!(error = %e, "passthrough recv_loop");
            }
        });

        Ok(Self {
            local_addr,
            shutdown,
            handle,
            _retry_signer: retry_signer,
            #[cfg(any(test, feature = "test-gauges"))]
            table,
        })
    }

    /// The socket address the listener is bound to.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Number of dispatch-table entries (test gauge). Each flow may
    /// hold up to 2 entries (client-DCID + LB-chosen SCID). The
    /// verify-gate (iv) bound is `2 * max_quic_connections`.
    #[cfg(any(test, feature = "test-gauges"))]
    #[must_use]
    pub fn flows_len(&self) -> usize {
        self.table.len()
    }

    /// Trigger graceful shutdown. Returns the listener task's
    /// `JoinHandle`.
    #[must_use]
    pub fn shutdown(self) -> tokio::task::JoinHandle<()> {
        self.shutdown.cancel();
        self.handle
    }
}

// ============================================================
// Retry-secret loader (mirrors listener.rs pattern; 32-byte secret).
// ============================================================

const RETRY_SECRET_LEN: usize = 32;

fn load_or_generate_retry_secret(path: &std::path::Path) -> std::io::Result<RetryTokenSigner> {
    match std::fs::read(path) {
        Ok(bytes) => {
            if bytes.len() != RETRY_SECRET_LEN {
                return Err(std::io::Error::other(format!(
                    "retry secret file {} has wrong length: expected {} bytes, got {}",
                    path.display(),
                    RETRY_SECRET_LEN,
                    bytes.len()
                )));
            }
            let mut secret = [0u8; RETRY_SECRET_LEN];
            secret.copy_from_slice(
                bytes
                    .get(..RETRY_SECRET_LEN)
                    .unwrap_or(&[0u8; RETRY_SECRET_LEN]),
            );
            Ok(RetryTokenSigner::new_with_secret(secret))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            let mut secret = [0u8; RETRY_SECRET_LEN];
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut secret)
                .map_err(|e| std::io::Error::other(format!("rng: {e}")))?;
            write_secret_file(path, &secret)?;
            Ok(RetryTokenSigner::new_with_secret(secret))
        }
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn write_secret_file(path: &std::path::Path, secret: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(secret)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret_file(path: &std::path::Path, secret: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::path::PathBuf;

    #[test]
    fn flow_entry_audit_compiles() {
        // Just constructing a FlowEntry + invoking the audit fn proves
        // the destructuring pattern stays in sync with the struct
        // definition. Compile-time enforcement.
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(32);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        let sock = runtime.block_on(async {
            UdpSocket::bind(SocketAddr::new(
                std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
                0,
            ))
            .await
            .expect("bind")
        });
        let fe = FlowEntry {
            backend: SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 1234),
            short_dcid_len: AtomicUsize::new(0),
            last_seen_ms: AtomicU64::new(0),
            peer: PlMutex::new(SocketAddr::new(
                std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
                5678,
            )),
            backend_sock: Arc::new(sock),
            backlog_tx: tx,
            closed: CancellationToken::new(),
            #[cfg(any(test, feature = "test-gauges"))]
            dropped: Arc::new(AtomicBool::new(false)),
        };
        _flow_entry_field_audit(&fe);
        // Sanity touch+set+get.
        let epoch = Instant::now();
        fe.touch(epoch, epoch);
        assert_eq!(fe.last_seen_ms.load(Ordering::Relaxed), 0);
        let new_peer = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 9999);
        fe.set_peer(new_peer);
        assert_eq!(fe.get_peer(), new_peer);
    }

    #[test]
    fn retry_packet_byte_layout() {
        // Smoke-test the hand-rolled Retry writer. RFC 9001 §A.4 has
        // a worked Retry example with known inputs/outputs — for the
        // gate-1 build we just check the layout invariants here; the
        // 1000-case differential vs quiche::retry lands in
        // tests/passthrough_retry_differential.rs in the next edit.
        let odcid = [0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];
        let client_scid: [u8; 0] = [];
        let new_scid = [0xaau8; LB_SCID_LEN];
        let version = 0x0000_0001u32;
        let token = b"opaque-retry-token";
        let mut out = Vec::new();
        build_retry_packet(&odcid, &client_scid, &new_scid, version, token, &mut out)
            .expect("build_retry_packet OK");
        // Layout: byte0(1) + version(4) + dcid_len(1) + dcid(0) +
        // scid_len(1) + scid(16) + token(18) + tag(16).
        // (dcid len = 0 bytes here so no DCID-bytes term.)
        assert_eq!(out.len(), 1 + 4 + 1 + 1 + 16 + 18 + 16);
        assert_eq!(*out.first().unwrap_or(&0), 0b1111_0000);
        assert_eq!(out.get(1..5).unwrap_or(&[]), &version.to_be_bytes());
        assert_eq!(out.get(5).copied().unwrap_or(0xff), 0u8); // DCID len = 0
        assert_eq!(out.get(6).copied().unwrap_or(0xff), 16u8); // SCID len = 16
        assert_eq!(out.get(7..23).unwrap_or(&[]), &new_scid);
        assert_eq!(out.get(23..41).unwrap_or(&[]), token.as_slice());
        // Tag is last 16 bytes; deterministic for the same inputs.
        let tag = out.get(out.len() - 16..).unwrap_or(&[]);
        assert_eq!(tag.len(), 16);
    }

    #[test]
    fn retry_packet_deterministic() {
        // Same inputs → same bytes (no randomness in the writer).
        let odcid = [1u8, 2, 3, 4];
        let cscid = [5u8, 6, 7];
        let nscid = [9u8; LB_SCID_LEN];
        let token = b"t";
        let mut a = Vec::new();
        let mut b = Vec::new();
        build_retry_packet(&odcid, &cscid, &nscid, 1, token, &mut a).unwrap();
        build_retry_packet(&odcid, &cscid, &nscid, 1, token, &mut b).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn passthrough_params_defaults_match_owner_rulings() {
        let p = PassthroughParams::new(
            SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            vec![],
            PathBuf::from("/tmp/retry.bin"),
        );
        assert_eq!(p.max_quic_connections, 100_000); // ruling §9.4
        assert_eq!(p.min_client_dcid_len, 8); // ruling §9.3
        assert!(!p.strict_source_binding); // ruling §9.1'
        assert_eq!(p.per_flow_backlog, 32);
        assert_eq!(p.max_dcid_len_routed, MAX_CID_LEN);
    }

    // ========================================================
    // S15 A3 — threat-defence + observability coverage tests.
    //
    // These drive handle_initial / forward_short / evict_oldest /
    // load_or_generate_retry_secret / audit_allow directly through an
    // in-crate RouterCtx so the private branches (mint_retry true/false,
    // verify reject, DCID floor, multi-length fallback, error/eviction
    // edges at the lcov-uncovered 599-674 / 787-848 / 994-1037 clusters)
    // are exercised. Lifts passthrough.rs cov 75.91% -> >=80%.
    // ========================================================

    use lb_observability::{MetricsRegistry, PassthroughMetrics};

    const T_SECRET: [u8; RETRY_SECRET_LEN] = [0x5au8; RETRY_SECRET_LEN];

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    /// Build an in-crate [`RouterCtx`] for unit tests: a real loopback
    /// `TokioUdp` dataplane (so `send_to` works), one bound-but-idle
    /// backend, fresh metrics, and a deterministic retry signer. Returns
    /// the ctx plus the backend's bound addr.
    async fn test_ctx(
        mut mutate: impl FnMut(&mut PassthroughParams),
    ) -> (Arc<RouterCtx>, PassthroughMetrics, SocketAddr) {
        // A bound backend socket so the per-flow forward task has a
        // reachable destination (it just discards what it receives).
        let backend = UdpSocket::bind(loopback(0)).await.expect("backend bind");
        let backend_addr = backend.local_addr().expect("backend addr");

        let dataplane = select_dataplane(loopback(0), TierPolicy::Auto)
            .await
            .expect("dataplane");

        let registry = MetricsRegistry::new();
        let metrics = PassthroughMetrics::register(&registry).expect("metrics");

        let mut params = PassthroughParams::new(loopback(0), vec![backend_addr], PathBuf::new());
        params.metrics = Some(metrics.clone());
        mutate(&mut params);

        let backends: Vec<Backend> = params
            .backends
            .iter()
            .enumerate()
            .map(|(i, sa)| Backend {
                id: format!("backend-{i}-{sa}"),
                weight: 1,
                active_connections: 0,
                active_requests: 0,
                latency_ewma_ns: 0,
                state: None,
            })
            .collect();
        let maglev = Maglev::new(&backends).expect("maglev");

        let ctx = Arc::new(RouterCtx {
            params,
            maglev,
            backends,
            retry_signer: Arc::new(RetryTokenSigner::new_with_secret(T_SECRET)),
            table: Arc::new(DashMap::new()),
            listener_sock: dataplane,
            epoch: Instant::now(),
            audit_last_source_binding_ms: AtomicU64::new(AUDIT_NEVER),
            audit_last_cap_hit_ms: AtomicU64::new(AUDIT_NEVER),
        });
        (ctx, metrics, backend_addr)
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt")
    }

    // --- (1) min_client_dcid_len floor (table-driven) ---------------

    #[test]
    fn min_client_dcid_len_floor_table() {
        // (dcid_len, floor, expect_inserted)
        let cases = [
            (4usize, 8usize, false), // below floor → dropped
            (7, 8, false),           // below floor → dropped
            (8, 8, true),            // at floor → proceeds
            (12, 8, true),           // above floor → proceeds
            (8, 12, false),          // raised floor → dropped
        ];
        rt().block_on(async {
            for (dcid_len, floor, expect_inserted) in cases {
                // mint_retry=false so an at/above-floor no-token Initial
                // builds a flow (the verbatim-forward branch) rather than
                // returning early on the Retry-mint branch.
                let (ctx, _m, _b) = test_ctx(|p| {
                    p.min_client_dcid_len = floor;
                    p.mint_retry = false;
                })
                .await;
                let dcid = vec![0xABu8; dcid_len];
                handle_initial(
                    Arc::clone(&ctx),
                    vec![0u8; 8],
                    loopback(40000),
                    1,
                    &dcid,
                    &[],
                    None,
                )
                .await;
                assert_eq!(
                    ctx.table.contains_key(dcid.as_slice()),
                    expect_inserted,
                    "dcid_len={dcid_len} floor={floor}"
                );
            }
        });
    }

    // --- (2) mint_retry = true → stateless Retry minted -------------

    #[test]
    fn mint_retry_true_mints_and_does_not_insert() {
        rt().block_on(async {
            // A sibling socket plays the "client": handle_initial sends the
            // Retry back to `from`, which is this socket's addr.
            let client = UdpSocket::bind(loopback(0)).await.expect("client bind");
            let from = client.local_addr().expect("client addr");

            let (ctx, m, _b) = test_ctx(|p| p.mint_retry = true).await;
            let dcid = vec![0x11u8; 8];
            handle_initial(
                Arc::clone(&ctx),
                vec![0u8; 8],
                from,
                1,
                &dcid,
                &[0x22u8; 8],
                None,
            )
            .await;

            // No flow allocated — Retry is stateless.
            assert!(ctx.table.is_empty(), "Retry-mint must not insert a flow");
            assert_eq!(m.retry_minted_total.get(), 1, "one Retry minted");
            // The Retry packet landed on the client socket.
            let mut buf = [0u8; 256];
            let n = tokio::time::timeout(Duration::from_secs(2), client.recv(&mut buf))
                .await
                .expect("retry recv timeout")
                .expect("retry recv");
            assert!(n > 0, "Retry packet received");
            assert_eq!(buf[0], 0b1111_0000, "byte0 = Retry long-header");
        });
    }

    // --- (3) mint_retry = false → forward verbatim, flow created ----

    #[test]
    fn mint_retry_false_forwards_and_inserts() {
        rt().block_on(async {
            let (ctx, m, _b) = test_ctx(|p| p.mint_retry = false).await;
            let dcid = vec![0x33u8; 8];
            handle_initial(
                Arc::clone(&ctx),
                vec![0u8; 8],
                loopback(40001),
                1,
                &dcid,
                &[],
                None,
            )
            .await;
            assert!(ctx.table.contains_key(dcid.as_slice()), "flow inserted");
            assert_eq!(
                m.retry_minted_total.get(),
                0,
                "no Retry minted when mint_retry=false"
            );
            assert_eq!(m.flows.get(), 1, "flows gauge tracks the new flow");
        });
    }

    // --- (4) token-present verify: reject vs accept -----------------

    #[test]
    fn token_verify_reject_then_accept() {
        rt().block_on(async {
            // (a) garbage token → reject + counter, no flow.
            let (ctx, m, _b) = test_ctx(|_| {}).await;
            let dcid = vec![0x44u8; 8];
            handle_initial(
                Arc::clone(&ctx),
                vec![0u8; 8],
                loopback(40002),
                1,
                &dcid,
                &[],
                Some(&[0xDEu8; 16]),
            )
            .await;
            assert!(ctx.table.is_empty(), "rejected token must not insert");
            assert_eq!(m.retry_rejected_total.get(), 1, "one verify rejection");

            // (b) a validly-minted token → accept + flow created.
            let (ctx2, m2, _b2) = test_ctx(|_| {}).await;
            let from = loopback(40003);
            let dcid2 = vec![0x55u8; 8];
            let good = ctx2.retry_signer.mint(from, &dcid2);
            handle_initial(
                Arc::clone(&ctx2),
                vec![0u8; 8],
                from,
                1,
                &dcid2,
                &[],
                Some(&good),
            )
            .await;
            assert!(
                ctx2.table.contains_key(dcid2.as_slice()),
                "valid token accepted"
            );
            assert_eq!(
                m2.retry_rejected_total.get(),
                0,
                "no rejection on a valid token"
            );
        });
    }

    // --- (5) eviction + negative control ----------------------------

    #[test]
    fn evict_oldest_at_cap_and_negative_control() {
        rt().block_on(async {
            // cap=1 → dispatch-table bound = 2 (2*cap). Insert 2 distinct
            // flows via mint_retry=false (one client-DCID key each); the
            // 3rd triggers a cap-hit eviction.
            let (ctx, m, _b) = test_ctx(|p| {
                p.max_quic_connections = 1;
                p.mint_retry = false;
            })
            .await;
            for i in 0u8..3 {
                let dcid = vec![0x60 + i; 8];
                handle_initial(
                    Arc::clone(&ctx),
                    vec![0u8; 8],
                    loopback(41000 + u16::from(i)),
                    1,
                    &dcid,
                    &[],
                    None,
                )
                .await;
            }
            assert!(ctx.table.len() <= 2, "table bounded at 2*cap");
            assert!(
                m.flows_evicted_total.get() >= 1,
                "at least one eviction observed"
            );
            assert_eq!(
                m.flows.get() as usize,
                ctx.table.len(),
                "gauge == table size"
            );

            // Negative control: cap=4, only 3 opens → no eviction.
            let (ctx2, m2, _b2) = test_ctx(|p| {
                p.max_quic_connections = 4;
                p.mint_retry = false;
            })
            .await;
            for i in 0u8..3 {
                let dcid = vec![0x70 + i; 8];
                handle_initial(
                    Arc::clone(&ctx2),
                    vec![0u8; 8],
                    loopback(42000 + u16::from(i)),
                    1,
                    &dcid,
                    &[],
                    None,
                )
                .await;
            }
            assert_eq!(m2.flows_evicted_total.get(), 0, "no eviction under cap");
            assert_eq!(ctx2.table.len(), 3, "all three flows resident");
        });
    }

    // --- (5b) F-S20-2 idle sweep + reclamation proof + negative control ---

    #[test]
    fn idle_sweep_reclaims_idle_flows_and_frees_them() {
        rt().block_on(async {
            let (ctx, m, _b) = test_ctx(|p| p.mint_retry = false).await;
            // Open 3 flows (one client-DCID key each). Each spawns a forward
            // + reverse pump and pins a backend UDP socket fd.
            for i in 0u8..3 {
                let dcid = vec![0x80 + i; 8];
                handle_initial(
                    Arc::clone(&ctx),
                    vec![0u8; 8],
                    loopback(45000 + u16::from(i)),
                    1,
                    &dcid,
                    &[],
                    None,
                )
                .await;
            }
            assert_eq!(ctx.table.len(), 3, "3 flows resident");

            // Capture each flow's Drop-gauge WITHOUT holding the FlowEntry Arc
            // (holding it would prevent reclamation), so we can prove the
            // entries are actually freed (fd + tasks) after the sweep.
            let dropped_flags: Vec<Arc<AtomicBool>> = ctx
                .table
                .iter()
                .map(|kv| Arc::clone(&kv.value().dropped))
                .collect();

            // Negative control: a generous idle window leaves freshly-touched
            // flows resident (their last_seen is ~now).
            assert_eq!(
                sweep_idle_flows(&ctx, 10_000),
                0,
                "fresh flows must NOT be reaped under a 10s idle window"
            );
            assert_eq!(ctx.table.len(), 3, "negative control: all resident");
            assert_eq!(m.flows_evicted_total.get(), 0, "no eviction yet");

            // Make every flow look idle (last_seen far in the past), let a
            // couple ms elapse so the epoch-relative now is non-zero, then
            // sweep with a 1ms idle window.
            for kv in ctx.table.iter() {
                kv.value().last_seen_ms.store(0, Ordering::Relaxed);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
            let reaped = sweep_idle_flows(&ctx, 1);
            assert_eq!(reaped, 3, "all 3 idle flows reaped");
            assert!(ctx.table.is_empty(), "table empty after idle sweep");
            assert_eq!(
                m.flows_evicted_total.get(),
                3,
                "one eviction event per reclaimed flow"
            );
            assert_eq!(m.flows.get(), 0, "gauge reflects empty table");

            // Reclamation proof (the load-bearing part of F-S20-2): the
            // per-flow reverse pump must EXIT on the cancel so the FlowEntry's
            // strong count reaches zero — its Drop fires (closing the backend
            // UDP socket fd + the forward task's channel). Poll the Drop gauge.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let all = dropped_flags.iter().all(|d| d.load(Ordering::Acquire));
                if all {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "idle-swept flows must be FREED (Drop fires) — a lingering \
                     reverse pump would leak the fd (the F-S20-2 mechanism)"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
    }

    // --- (6) forward_short multi-length fallback + strict-source -----

    #[test]
    fn forward_short_via_strict_source_table() {
        rt().block_on(async {
            // (strict_source_binding, peer_match, expect_forward)
            let cases = [
                (false, false, true), // off → always forward
                (false, true, true),  // off → always forward
                (true, true, true),   // on + match → forward
                (true, false, false), // on + mismatch → DROP + audit
            ];
            for (strict, peer_match, expect_fwd) in cases {
                let (ctx, _m, backend) = test_ctx(|p| p.strict_source_binding = strict).await;
                let recorded = loopback(43000);
                let observed = if peer_match {
                    recorded
                } else {
                    loopback(43999)
                };
                let (tx, _rx) = mpsc::channel::<Vec<u8>>(8);
                let flow = FlowEntry {
                    backend,
                    short_dcid_len: AtomicUsize::new(0),
                    last_seen_ms: AtomicU64::new(0),
                    peer: PlMutex::new(recorded),
                    backend_sock: Arc::new(UdpSocket::bind(loopback(0)).await.expect("bind")),
                    backlog_tx: tx,
                    closed: CancellationToken::new(),
                    dropped: Arc::new(AtomicBool::new(false)),
                };
                assert_eq!(
                    forward_short_via(&ctx, &flow, &[], observed),
                    expect_fwd,
                    "strict={strict} match={peer_match}"
                );
            }
        });
    }

    #[test]
    fn forward_short_multi_length_fallback_hits() {
        rt().block_on(async {
            // Register a flow keyed by a 10-byte DCID with short_dcid_len=10,
            // then send a short-header packet whose default-len parse misses
            // but whose 10-byte prefix hits the multi-length fallback loop.
            let (ctx, _m, backend) = test_ctx(|p| p.max_dcid_len_routed = 8).await;
            let dcid10 = vec![0x80u8; 10];
            let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
            let flow = Arc::new(FlowEntry {
                backend,
                short_dcid_len: AtomicUsize::new(10),
                last_seen_ms: AtomicU64::new(0),
                peer: PlMutex::new(loopback(44000)),
                backend_sock: Arc::new(UdpSocket::bind(loopback(0)).await.expect("bind")),
                backlog_tx: tx,
                closed: CancellationToken::new(),
                dropped: Arc::new(AtomicBool::new(false)),
            });
            ctx.table.insert(dcid10.clone(), Arc::clone(&flow));

            // Short-header packet: byte0 (short) + 10-byte DCID + payload.
            let mut pkt = vec![0b0100_0000u8];
            pkt.extend_from_slice(&dcid10);
            pkt.extend_from_slice(&[0xEE, 0xEE]);
            // default_dcid is the 8-byte prefix (max_dcid_len_routed=8) which
            // is NOT a table key → forces the multi-length fallback.
            let default_dcid = pkt.get(1..9).expect("8-byte prefix").to_vec();
            forward_short(&ctx, &pkt, &default_dcid, loopback(44001)).await;

            assert!(
                rx.try_recv().is_ok(),
                "multi-length fallback forwarded the packet"
            );
        });
    }

    // --- (7) retry-secret loader edges (994-1037) -------------------

    #[test]
    fn retry_secret_loader_edges() {
        let dir = std::env::temp_dir().join(format!(
            "lb-passthrough-a3-secret-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");

        // NotFound → generates a fresh 32-byte secret + file (mode 0600).
        let gen_path = dir.join("nested").join("retry.bin");
        let _ = load_or_generate_retry_secret(&gen_path).expect("generate");
        let written = std::fs::read(&gen_path).expect("read back");
        assert_eq!(written.len(), RETRY_SECRET_LEN, "generated secret length");

        // Existing correct-length file → Ok.
        let _ = load_or_generate_retry_secret(&gen_path).expect("load existing");

        // Wrong-length file → Err.
        let bad = dir.join("bad.bin");
        std::fs::write(&bad, [0u8; 10]).expect("write bad");
        assert!(
            load_or_generate_retry_secret(&bad).is_err(),
            "wrong-length rejected"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- (8) pick_backend / hash determinism ------------------------

    #[test]
    fn pick_backend_is_deterministic() {
        rt().block_on(async {
            let (ctx, _m, _b) = test_ctx(|_| {}).await;
            let dcid = [0x90u8; 8];
            let a = pick_backend(&ctx, &dcid);
            let b = pick_backend(&ctx, &dcid);
            assert_eq!(a, b, "same DCID → same backend");
            assert!(a.is_some(), "one backend configured → Some");
        });
        // hash mixing is non-trivial for non-empty input.
        assert_ne!(hash_dcid_for_maglev(&[1, 2, 3]), 0);
        assert_eq!(
            hash_dcid_for_maglev(&[1, 2, 3]),
            hash_dcid_for_maglev(&[1, 2, 3]),
            "deterministic"
        );
    }

    // --- (9) audit_allow throttle gate (explicit clock) -------------

    #[test]
    fn audit_allow_one_per_window() {
        let slot = AtomicU64::new(AUDIT_NEVER);
        let window = Duration::from_secs(60);
        // First event always emits.
        assert!(audit_allow(&slot, 0, window), "first event emits");
        // Within the window: suppressed.
        assert!(!audit_allow(&slot, 100, window), "in-window suppressed");
        assert!(
            !audit_allow(&slot, 59_999, window),
            "just-before-window suppressed"
        );
        // At/after the window: emits again.
        assert!(audit_allow(&slot, 60_000, window), "post-window emits");
        // And re-throttles from the new mark.
        assert!(
            !audit_allow(&slot, 60_001, window),
            "re-throttled after re-emit"
        );
    }
}
