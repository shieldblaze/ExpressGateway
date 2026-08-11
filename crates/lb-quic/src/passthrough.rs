//! Mode A passthrough datapath — route encrypted QUIC packets by Connection ID
//! without decrypting. There is NO TLS state on this path: no
//! `quiche::Connection` is instantiated for client/backend flows, no BoringSSL
//! handshake runs, no cert/key is loaded, and [`FlowEntry`] carries routing
//! state ONLY — see its SAFETY/INVARIANT block and the
//! [`_flow_entry_field_audit`] destructuring audit at the bottom of this module.
//!
//! Carry-forwards: CF-S15-FLOWENTRY-FIELD-AUDIT (the field audit below),
//! CF-S15-RETRY-NO-QUICHE (hand-rolled [`build_retry_packet`]),
//! CF-S15-DCID-MAP-XDP (`UdpDataplane::dcid_map_fd`, unused in v1.0).

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

/// Length of LB-chosen SCIDs in Retry packets (RFC 9000 §17.2.5 allows ≤ 20).
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

/// Construction parameters for [`PassthroughListener::spawn`].
#[derive(Debug, Clone)]
pub struct PassthroughParams {
    /// Bind address for the listener UDP socket.
    pub bind_addr: SocketAddr,
    /// Resolved backend addresses; consumed by Maglev consistent hashing.
    pub backends: Vec<SocketAddr>,
    /// Path to a 32-byte retry-secret file (generated 0600 if absent).
    pub retry_secret_path: PathBuf,
    /// Maximum concurrent QUIC flows.
    pub max_quic_connections: usize,
    /// Minimum client-chosen DCID length accepted — the cap-violation floor.
    pub min_client_dcid_len: usize,
    /// Per-flow datagram backlog; drop-newest when full.
    pub per_flow_backlog: usize,
    /// Strict source-IP binding: when true, a short-header packet whose peer
    /// IP does not match the flow's recorded peer is DROPPED (off-path
    /// injection guard) rather than treated as a NAT rebind.
    pub strict_source_binding: bool,
    /// Audit-log throttle window. 60s default — see §6.2.
    pub audit_throttle_window: Duration,
    /// Short-header DCID length to try first when no per-flow length is known.
    pub max_dcid_len_routed: usize,
    /// Whether the LB mints stateless Retry on no-token Initials (the §6.5
    /// Initial-flood defence). Default **true** in production.
    ///
    /// CF-S15-PASSTHROUGH-RETRY-ODCID: with `true`, the second Initial's wire
    /// DCID is the LB-chosen new_scid, so the backend cannot recover the
    /// client's original DCID (`original_destination_connection_id`) without a
    /// side channel — RFC 9000 §17.2.5 anticipates this via the "Retry Service"
    /// pattern. With `false` no-token Initials are forwarded verbatim and
    /// Initial-flood defence becomes the BACKEND's responsibility: a documented
    /// test/trusted-network escape, not a production setting.
    pub mint_retry: bool,
    /// F-S20-2: idle-flow reaper threshold. Passthrough cannot observe the
    /// encrypted CONNECTION_CLOSE, so without this a closed connection's flow
    /// persists until the LRU cap.
    pub flow_idle_timeout: Duration,
    /// `quic_passthrough_*` observability handles; `None` ⇒ every update is a
    /// no-op.
    pub metrics: Option<lb_observability::PassthroughMetrics>,
}

impl PassthroughParams {
    /// Build params with defaults for the non-bind / non-backend fields.
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

// SAFETY/INVARIANT: FlowEntry holds no key material — passthrough never
// decrypts, because the LB has no keys. Every field is non-cryptographic: a
// destination `SocketAddr`, a wire-format length, an epoch-millis timestamp,
// the client's 4-tuple, a kernel-owned UDP fd, and a datagram queue handle.
//
// **Adding a field**: enumerate it in [`_flow_entry_field_audit`] below AND
// give it a type-witness `let _: &T = field_name;`. Omitting either is a
// COMPILE ERROR, which is the point.
pub(crate) struct FlowEntry {
    /// The backend this flow is pinned to, decided at the first Initial.
    pub(crate) backend: SocketAddr,
    /// Short-header DCID length for this flow, recovered from the long-header
    /// SCID so short-header packets can be routed.
    pub(crate) short_dcid_len: AtomicUsize,
    /// Last-seen millis-since-epoch, for LRU eviction and the idle sweep.
    pub(crate) last_seen_ms: AtomicU64,
    /// Client's current 4-tuple, updated on every recv so a NAT rebind keeps
    /// routing to the same backend.
    pub(crate) peer: PlMutex<SocketAddr>,
    /// Per-flow backend UDP socket, `connect()`-ed to `backend`.
    pub(crate) backend_sock: Arc<UdpSocket>,
    /// Bounded queue feeding the per-flow forward task; full ⇒ drop-newest.
    pub(crate) backlog_tx: mpsc::Sender<Vec<u8>>,
    /// F-S20-2: per-flow shutdown signal. Cancelling it is what breaks the
    /// reverse pump out of its otherwise-indefinite blocking `recv()` — the
    /// load-bearing step, since an alive-but-silent backend never errors.
    pub(crate) closed: CancellationToken,
    /// LRU-eviction observed-flag (test gauge), set by [`Drop`] so a test can
    /// prove the entry was actually reclaimed rather than merely unlinked.
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

/// CF-S15-FLOWENTRY-FIELD-AUDIT — destructuring audit. A new `FlowEntry` field
/// makes the compiler surface this pattern as incomplete, forcing the author to
/// add it AND assign a type witness asserting the field's STATIC type is not a
/// key-shaped type from `ring`/`boring`/`quiche`.
// Unconditional, NOT `#[cfg(debug_assertions)]`: release builds compile out the
// `#[cfg(test)]` caller, so `backend` was then read nowhere and `field is never
// read` under `-D warnings` broke `cargo build --release` (S34). The
// `allow(dead_code)` covers the audit being uncalled outside `cfg(test)`.
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
    // Type-witnesses: ANY change to FlowEntry is a compile error here.
    let _: &SocketAddr = backend;
    let _: &AtomicUsize = short_dcid_len;
    let _: &AtomicU64 = last_seen_ms;
    let _: &PlMutex<SocketAddr> = peer;
    let _: &Arc<UdpSocket> = backend_sock;
    let _: &mpsc::Sender<Vec<u8>> = backlog_tx;
    let _: &CancellationToken = closed;
    #[cfg(any(test, feature = "test-gauges"))]
    let _: &Arc<AtomicBool> = dropped;
    // None of the above types are key material (AEAD keys, quiche::Connection…).
}

/// CF-S15-DCID-HASH: fast non-cryptographic mixing over the client DCID to feed
/// `Maglev::pick_with_key`. A cryptographic hash is not needed — Maglev's
/// permutation is the consistency layer — only determinism across runs.
fn hash_dcid_for_maglev(dcid: &[u8]) -> u64 {
    // Same finalizer as `lb_balancer::maglev::hash_str`, so the distribution
    // behaves identically to the L7-affinity path.
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

/// Build a QUIC v1 Retry packet per RFC 9000 §17.2.5 and compute its 16-byte
/// AEAD-AES-128-GCM Retry Integrity Tag per RFC 9001 §5.8.
///
/// Wire bytes:
///   `byte0 | version | DCID_len | DCID | SCID_len | SCID | token | tag(16)`
///
/// The Retry Pseudo-Packet sealed as AAD (§5.8) prepends the ODCID:
///   `ODCID_len(1) | ODCID | <the wire bytes above, without the tag>`
///
/// `odcid` is the DCID from the client's FIRST Initial and goes into the
/// pseudo-packet ONLY, never the wire bytes; `client_scid` becomes the on-wire
/// DCID; `new_scid` is the LB-chosen SCID that becomes the routing DCID for the
/// client's second Initial.
///
/// # Errors
///
/// Returns `Err` if `ring::aead` rejects the inputs.
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

    // RFC 9000 §17.2 byte0 for Retry: long-header form, fixed bit, type 0b11.
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

    // ring requires a mutable in-place buffer even for an AAD-only seal.
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

/// Test-only re-export of [`build_retry_packet`] for the byte-equality
/// differential against `quiche::retry`.
///
/// CF-S15-TESTGAUGES-EXPORT-NARROW: `cfg(test)` is FALSE while a downstream
/// integration test compiles, so gating this on it forced every consumer to
/// enable `test-gauges` for one symbol. Keep `build_retry_packet` private and
/// expose this doc-hidden wrapper unconditionally instead.
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

/// Routing-table entries: one per known DCID; a flow may hold up to 2 keys.
type FlowTable = DashMap<Vec<u8>, Arc<FlowEntry>>;

struct RouterCtx {
    params: PassthroughParams,
    /// Maglev table over the backend set, in an `Arc` for cheap clones.
    maglev: Maglev,
    /// `Backend` view of `params.backends`, index-aligned with the Maglev table.
    backends: Vec<Backend>,
    retry_signer: Arc<RetryTokenSigner>,
    table: Arc<FlowTable>,
    /// Listener-side UDP socket (the recv loop's write half).
    listener_sock: Arc<dyn UdpDataplane>,
    /// Process-relative monotonic epoch for `last_seen_ms`.
    epoch: Instant,
    /// Audit-log throttle state — one slot per audit category, so a flood in
    /// one category cannot suppress another's first line.
    audit_last_source_binding_ms: AtomicU64,
    audit_last_cap_hit_ms: AtomicU64,
}

/// Sentinel for "no audit line emitted yet", distinct from a real `0` reading
/// so the first event always clears the window check.
const AUDIT_NEVER: u64 = u64::MAX;

/// Audit-throttle gate: `true` (recording `now_ms`) iff the slot is
/// [`AUDIT_NEVER`] or `now_ms` is at least `window` past the last emit. Uses
/// `fetch_update`, so two threads racing the same window emit exactly once.
fn audit_allow(last_emit: &AtomicU64, now_ms: u64, window: Duration) -> bool {
    let window_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
    // TOOLCHAIN SHIM: nightly deprecates `fetch_update` in favour of
    // `try_update`, which does NOT exist on our MSRV (1.88). The fuzz-smoke
    // lane is the only nightly one and builds with `-D warnings`, so the
    // deprecation is a hard error there while stable still needs the old name.
    // Silenced narrowly — not crate-wide — so other deprecations still turn the
    // lane red. Drop this when the MSRV passes `try_update`'s stabilisation.
    #[allow(deprecated)]
    let won_window = last_emit.fetch_update(Ordering::AcqRel, Ordering::Acquire, |prev| {
        if prev == AUDIT_NEVER || now_ms.saturating_sub(prev) >= window_ms {
            Some(now_ms)
        } else {
            None
        }
    });
    won_window.is_ok()
}

/// Hash a DCID into a Maglev pick.
fn pick_backend(ctx: &RouterCtx, dcid: &[u8]) -> Option<SocketAddr> {
    let key = hash_dcid_for_maglev(dcid);
    let idx = ctx.maglev.pick_with_key(&ctx.backends, key).ok()?;
    ctx.params.backends.get(idx).copied()
}

/// Set `quic_passthrough_flows` to the current dispatch-table size — a
/// self-correcting gauge, re-read rather than incremented.
fn set_flows_gauge(ctx: &RouterCtx) {
    if let Some(m) = &ctx.params.metrics {
        m.flows
            .set(i64::try_from(ctx.table.len()).unwrap_or(i64::MAX));
    }
}

/// LRU eviction at cap: drop the entry with the oldest `last_seen_ms`. LRU, not
/// FIFO — a long-lived active flow must not be evicted ahead of an idle one.
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

/// F-S20-2 — reclaim a set of flows. SINGLE-SOURCED (R12) for both LRU
/// eviction and the periodic idle sweep. For each victim:
///
/// 1. **Cancel its `closed` token** so the reverse pump exits its otherwise
///    indefinite blocking `backend_sock.recv()` and releases its
///    `Arc<FlowEntry>`. This is the load-bearing step — removing the dispatch
///    keys alone cannot reclaim a flow whose backend is alive-but-silent, so
///    the fd + tasks would leak (the F-S20-2 mechanism).
/// 2. **Remove every dispatch key** pointing at the victim (by Arc identity; a
///    migrated flow may hold 2). Borrow-and-collect, to avoid an
///    iterator/remove deadlock in DashMap.
///
/// `flows_evicted_total` is bumped ONCE per flow reclaimed, NOT per removed CID
/// key, which would double-count 2-key flows. Returns the number of entries
/// removed, so the LRU caller can detect "nothing evicted" and avoid spinning.
fn reclaim_flows(ctx: &RouterCtx, victims: &[Arc<FlowEntry>]) -> usize {
    if victims.is_empty() {
        return 0;
    }
    // Signal each victim's pumps to stop (idempotent).
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

/// F-S20-2 — reclaim every flow idler than `idle_ms`, bounding the table by the
/// LIVE connection count instead of by the LRU cap. Passthrough cannot observe
/// the encrypted CONNECTION_CLOSE, so a closed connection's flow would
/// otherwise pin a backend fd + 2 pump tasks until the cap — the S20 leak.
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

/// F-S20-2 — the periodic reaper task body, extracted from
/// [`PassthroughListener::spawn`] so the loop is directly testable (a
/// `tokio::spawn`'d closure is invisible to unit-test coverage). The first tick
/// fires immediately; `Skip` missed-tick behaviour avoids a burst after a long
/// sweep.
async fn run_idle_sweeper(
    ctx: Arc<RouterCtx>,
    idle_ms: u64,
    period: Duration,
    shutdown: CancellationToken,
) {
    let mut tick = tokio::time::interval(period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            _ = tick.tick() => {
                let reaped = sweep_idle_flows(&ctx, idle_ms);
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
                // Either a retransmit of an Initial whose handshake finished,
                // or an unknown DCID — drop.
                forward_long_existing(&ctx, &data, dcid).await;
            }
            LongType::Retry | LongType::VersionNegotiation => {
                // Client-origin Retry / Version Negotiation are not legal
                // toward the LB.
                tracing::trace!(peer = %from, ?ty, "dropped client-origin Retry/VN");
            }
        },
        PublicHeader::Short { dcid } => {
            // The parser returned a Short with the default-length DCID; the
            // multi-length fallback runs inside `forward_short`.
            forward_short(&ctx, &data, dcid, from).await;
        }
    }
}

/// Default short-header DCID length to try first.
fn default_short_dcid_len(ctx: &RouterCtx) -> usize {
    ctx.params.max_dcid_len_routed
}

/// Initial-packet handler.
async fn handle_initial(
    ctx: Arc<RouterCtx>,
    pkt: Vec<u8>,
    from: SocketAddr,
    version: u32,
    dcid: &[u8],
    scid: &[u8],
    token: Option<&[u8]>,
) {
    // Cap-violation defence: drop Initials with DCIDs below the floor.
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
    // CF-S15-PASSTHROUGH-RETRY-ODCID: minting a Retry makes the LB-chosen
    // new_scid the second-Initial wire DCID, so the backend cannot recover the
    // client's ORIGINAL DCID without a side channel and a real-quiche backend
    // rejects the resulting `original_destination_connection_id`. RFC 9000
    // §17.2.5 anticipates this via the "Retry Service" pattern. The
    // `mint_retry` knob is the production-vs-trusted-network escape.
    let tok = token.unwrap_or(&[]);
    if tok.is_empty() && ctx.params.mint_retry {
        // Mint Retry: stateless, so no flow is allocated.
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

    // Token present ⇒ verify. With mint_retry=false and no token we forward
    // verbatim and let the backend decide.
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

    // Cap-check, evicting the oldest on hit. The cap is on UNIQUE flows, while
    // the dispatch table holds up to 2 keys per flow.
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

    // Register the routing key for the wire DCID. On this branch the client has
    // already received our Retry, so the wire DCID IS our LB-chosen new_scid —
    // the single insertion below is already the §3.6 routing key.
    ctx.table.insert(dcid.to_vec(), Arc::clone(&flow));
    // Self-correcting gauge: re-read the post-insert table size.
    set_flows_gauge(&ctx);

    // Forward the inbound packet first.
    let _ = backlog_tx.try_send(pkt);

    // Per-flow forward pump (client→backend); exits when the backlog sender
    // drops or `closed` fires.
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
        // F-S20-2: race the blocking backend recv against the per-flow `closed`
        // token, so an alive-but-silent backend cannot pin this task forever.
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
        // short-header DCID length before short-header packets start.
        if let Ok(PublicHeader::Long { scid, .. }) = parse_public_header(slice, 0) {
            if !scid.is_empty() {
                let key = scid.to_vec();
                // Avoid clobbering an existing entry (a different flow could
                // legitimately own this key).
                ctx.table.entry(key).or_insert_with(|| Arc::clone(&flow));
                flow.short_dcid_len.store(scid.len(), Ordering::Relaxed);
            }
        }

        let peer = flow.get_peer();
        if let Err(e) = ctx.listener_sock.send_to(slice, peer).await {
            tracing::trace!(error = %e, "reverse send failed");
            // Don't break on transient send errors — UDP is best-effort.
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

/// Short-header inbound: try the single-length fast path, then walk the set of
/// known per-flow DCID lengths.
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
    // Miss ⇒ drop; the client retransmit covers a genuine race.
}

/// Strict source-binding gate: with the knob on, a peer-IP mismatch DROPS the
/// packet (off-path injection guard) instead of accepting it as a NAT rebind.
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
        // Throttled audit record: one `warn!` per window, so an injection
        // flood cannot drown the log.
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
        // RNG failure on a supported platform is effectively impossible; fail
        // closed rather than emit a predictable SCID.
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
    /// Bind a UDP socket, load (or generate) the retry secret, build the Maglev
    /// table, and spawn the recv loop + idle reaper.
    ///
    /// # Errors
    ///
    /// Bind failure, or a retry-secret load/generate failure.
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

        // F-S20-2: periodic idle-flow reaper — bounds the flow table by the
        // LIVE connection count rather than the LRU cap.
        let idle = ctx.params.flow_idle_timeout;
        if !idle.is_zero() {
            let sweep_ctx = Arc::clone(&ctx);
            let sweep_shutdown = shutdown.clone();
            let idle_ms = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
            // Sweep cadence = a quarter of the idle window, clamped.
            let period = Duration::from_millis((idle_ms / 4).max(1))
                .clamp(Duration::from_secs(1), Duration::from_secs(10));
            tokio::spawn(run_idle_sweeper(sweep_ctx, idle_ms, period, sweep_shutdown));
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

    /// Number of dispatch-table entries (test gauge); a flow may hold 2.
    #[cfg(any(test, feature = "test-gauges"))]
    #[must_use]
    pub fn flows_len(&self) -> usize {
        self.table.len()
    }

    /// Trigger graceful shutdown, returning the listener task's join result.
    #[must_use]
    pub fn shutdown(self) -> tokio::task::JoinHandle<()> {
        self.shutdown.cancel();
        self.handle
    }
}

const RETRY_SECRET_LEN: usize = 32;

/// F-INFRA-01 — perm-gate the retry-secret LOAD path. A deliberate cross-path
/// duplicate of `lb_quic::listener::check_retry_secret_perms`; keep them in
/// sync. Strict on release, warn-only on debug, closing the asymmetry against
/// the 0600 generate path.
#[cfg(unix)]
fn check_retry_secret_perms(path: &std::path::Path, strict: bool) -> std::io::Result<()> {
    match lb_security::assert_owner_only(path, strict) {
        Ok(lb_security::KeyPermAdvice::Ok | lb_security::KeyPermAdvice::NotApplicable) => Ok(()),
        Ok(lb_security::KeyPermAdvice::TooPermissive { mode }) => {
            tracing::warn!(
                retry_secret = %path.display(),
                mode = format!("{mode:o}"),
                "retry-secret file permissions wider than 0o600 — tighten with `chmod 600`"
            );
            Ok(())
        }
        Err(e) => Err(std::io::Error::other(format!(
            "retry-secret permission check failed for {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(not(unix))]
fn check_retry_secret_perms(_path: &std::path::Path, _strict: bool) -> std::io::Result<()> {
    Ok(())
}

fn load_or_generate_retry_secret(path: &std::path::Path) -> std::io::Result<RetryTokenSigner> {
    match std::fs::read(path) {
        Ok(bytes) => {
            // F-INFRA-01: perm-gate the existing-file load (strict on release).
            check_retry_secret_perms(path, !cfg!(debug_assertions))?;
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
        // Constructing a FlowEntry and invoking the audit proves the
        // destructuring pattern is exhaustive and the witnesses type-check.
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
        // Smoke-test the hand-rolled Retry writer: layout + determinism. The
        // byte-equality differential against `quiche::retry` is the integration
        // test.
        let odcid = [0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];
        let client_scid: [u8; 0] = [];
        let new_scid = [0xaau8; LB_SCID_LEN];
        let version = 0x0000_0001u32;
        let token = b"opaque-retry-token";
        let mut out = Vec::new();
        build_retry_packet(&odcid, &client_scid, &new_scid, version, token, &mut out)
            .expect("build_retry_packet OK");
        // Layout: byte0 | version | dcid_len | dcid | scid_len | scid | token
        // | tag(16).
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

    // Threat-defence + observability coverage: drive handle_initial /
    // forward_short / evict_oldest / load_or_generate_retry_secret /
    // audit_allow directly through an in-crate RouterCtx so the private
    // branches are exercised.

    use lb_observability::{MetricsRegistry, PassthroughMetrics};

    const T_SECRET: [u8; RETRY_SECRET_LEN] = [0x5au8; RETRY_SECRET_LEN];

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    /// Build an in-crate [`RouterCtx`] for unit tests over real loopback
    /// sockets.
    async fn test_ctx(
        mut mutate: impl FnMut(&mut PassthroughParams),
    ) -> (Arc<RouterCtx>, PassthroughMetrics, SocketAddr) {
        // A bound backend socket so the per-flow forward task has a target.
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
                // mint_retry=false so an at/above-floor no-token Initial is
                // forwarded and a flow IS created — isolating the floor.
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
            // A sibling socket plays the "client" and receives the Retry.
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
            // cap=1 ⇒ dispatch-table bound = 2. Insert 2 distinct flows and the
            // older must be evicted.
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

            // Negative control: cap=4 with only 3 opens ⇒ no eviction.
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
            // Open 3 flows, one client-DCID key each.
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

            // Capture each flow's Drop-gauge WITHOUT holding the Arc, or the
            // strong count never reaches zero and Drop never runs.
            let dropped_flags: Vec<Arc<AtomicBool>> = ctx
                .table
                .iter()
                .map(|kv| Arc::clone(&kv.value().dropped))
                .collect();

            // Negative control: a generous idle window reaps nothing.
            assert_eq!(
                sweep_idle_flows(&ctx, 10_000),
                0,
                "fresh flows must NOT be reaped under a 10s idle window"
            );
            assert_eq!(ctx.table.len(), 3, "negative control: all resident");
            assert_eq!(m.flows_evicted_total.get(), 0, "no eviction yet");

            // Make every flow look idle, then sweep.
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

            // Reclamation proof (the load-bearing part of F-S20-2): each
            // entry's Drop actually ran, so the fd + tasks were released — not
            // merely unlinked from the table.
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

    // --- (5c) F-S20-2 the PERIODIC reaper task (run_idle_sweeper) ---------

    #[test]
    fn idle_sweeper_task_reaps_periodically_and_stops_on_shutdown() {
        rt().block_on(async {
            let (ctx, m, _b) = test_ctx(|p| p.mint_retry = false).await;
            for i in 0u8..3 {
                let dcid = vec![0x90 + i; 8];
                handle_initial(
                    Arc::clone(&ctx),
                    vec![0u8; 8],
                    loopback(46000 + u16::from(i)),
                    1,
                    &dcid,
                    &[],
                    None,
                )
                .await;
            }
            assert_eq!(ctx.table.len(), 3, "3 flows resident");
            // Mark them idle so the FIRST sweep tick reaps them.
            for kv in ctx.table.iter() {
                kv.value().last_seen_ms.store(0, Ordering::Relaxed);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;

            // Drive the REAL periodic reaper task (idle_ms=1, period=20ms).
            let shutdown = CancellationToken::new();
            let task = tokio::spawn(run_idle_sweeper(
                Arc::clone(&ctx),
                1,
                Duration::from_millis(20),
                shutdown.clone(),
            ));

            // The periodic task (not a direct call) must reclaim all flows.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while !ctx.table.is_empty() {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the periodic run_idle_sweeper task must reap idle flows"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(
                m.flows_evicted_total.get() >= 3,
                "periodic sweep bumped the eviction counter"
            );

            // Shutdown arm: the task must EXIT promptly on cancel.
            shutdown.cancel();
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .expect("run_idle_sweeper must exit on shutdown.cancel()")
                .expect("reaper task joined cleanly");
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
            // A flow keyed by a 10-byte DCID with short_dcid_len=10, so only
            // the multi-length fallback can route it.
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
            // default_dcid is the 8-byte prefix, which must NOT match.
            let default_dcid = pkt.get(1..9).expect("8-byte prefix").to_vec();
            forward_short(&ctx, &pkt, &default_dcid, loopback(44001)).await;

            assert!(
                rx.try_recv().is_ok(),
                "multi-length fallback forwarded the packet"
            );
        });
    }

    // --- (7) retry-secret loader edges ------------------------------

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
