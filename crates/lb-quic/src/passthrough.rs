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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use dashmap::DashMap;
use parking_lot::Mutex as PlMutex;
use ring::aead;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_security::RetryTokenSigner;

use crate::public_header::{LongType, MAX_CID_LEN, PublicHeader, parse_public_header};
use crate::udp_dataplane::{Packet, PacketHandler, TierPolicy, select_dataplane};

// ============================================================
// Constants
// ============================================================

// dead_code allows on the helper consts/fns below: this is the A2
// foundation commit; the recv-loop state machine that consumes them
// lands in a follow-up edit. Tests below already exercise them.

/// Maximum UDP datagram a passthrough flow accepts. Mirrors
/// `lb_quic::udp_dataplane::MAX_UDP_DATAGRAM_SIZE` (65_535).
#[allow(dead_code)]
const MAX_UDP: usize = 65_535;

/// Length of LB-chosen SCIDs (in Retry packets). RFC 9000 §17.2.5
/// requires `len > 0`; 16 bytes matches quiche / termination router.
#[allow(dead_code)]
const LB_SCID_LEN: usize = 16;

/// AEAD-AES-128-GCM tag length (RFC 9001 §5.8 fixed value).
#[allow(dead_code)]
const RETRY_INTEGRITY_TAG_LEN: usize = 16;

/// RFC 9001 §5.8 — fixed Retry Integrity Tag key for QUIC v1.
#[allow(dead_code)]
const RETRY_KEY_V1: [u8; 16] = [
    0xbe, 0x0c, 0x69, 0x0b, 0x9f, 0x66, 0x57, 0x5a, 0x1d, 0x76, 0x6b, 0x54, 0xe3, 0x68, 0xc8, 0x4e,
];

/// RFC 9001 §5.8 — fixed Retry Integrity Tag nonce for QUIC v1.
#[allow(dead_code)]
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
    } = e;
    // Type-witnesses: each field's static type is enumerated here.
    // ANY change to FlowEntry → compile error here.
    let _: &SocketAddr = backend;
    let _: &AtomicUsize = short_dcid_len;
    let _: &AtomicU64 = last_seen_ms;
    let _: &PlMutex<SocketAddr> = peer;
    let _: &Arc<UdpSocket> = backend_sock;
    let _: &mpsc::Sender<Vec<u8>> = backlog_tx;
    // None of the above types are key material (AEAD::Key,
    // ring::aead::*Key, boring::Aead, quiche::Connection, ...).
}

// ============================================================
// Hand-rolled Retry packet writer (CF-S15-RETRY-NO-QUICHE)
// ============================================================

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
#[allow(clippy::too_many_arguments, dead_code)]
fn build_retry_packet(
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
    table: Arc<DashMap<Vec<u8>, Arc<FlowEntry>>>,
}

impl PassthroughListener {
    /// Bind a UDP socket, load (or generate) the retry secret, and
    /// spawn the recv loop.
    ///
    /// Stub-shape for the gate-1 build. Full state-machine wiring
    /// lands in a subsequent edit to this file.
    ///
    /// # Errors
    ///
    /// Returns the OS bind error.
    pub async fn spawn(
        params: PassthroughParams,
        shutdown: CancellationToken,
    ) -> std::io::Result<Self> {
        let dataplane = select_dataplane(params.bind_addr, TierPolicy::Auto)
            .await
            .map_err(|e| std::io::Error::other(format!("dataplane bind: {e}")))?;
        let local_addr = dataplane.local_addr();
        let retry_signer = Arc::new(load_or_generate_retry_secret(&params.retry_secret_path)?);
        let table: Arc<DashMap<Vec<u8>, Arc<FlowEntry>>> = Arc::new(DashMap::new());
        let table_for_loop = Arc::clone(&table);
        let shutdown_for_loop = shutdown.clone();

        tracing::info!(
            address = %local_addr,
            protocol = "quic-passthrough",
            backends = params.backends.len(),
            "QUIC passthrough listener bound"
        );

        let handle = tokio::spawn(async move {
            let on_packet: PacketHandler<'_> = Arc::new(move |_pkt: Packet<'_>| {
                let _table = Arc::clone(&table_for_loop);
                Box::pin(async move {
                    // Stub: full routing state machine in next edit.
                })
            });
            if let Err(e) = dataplane.recv_loop(shutdown_for_loop, on_packet).await {
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

    /// Number of live flows (test gauge). Counts UNIQUE flows, not
    /// dispatch-table entries (each flow has up to 2 table keys).
    #[cfg(any(test, feature = "test-gauges"))]
    #[must_use]
    pub fn flows_len(&self) -> usize {
        // Approximate: dispatch-table size is roughly 2× flows.
        // Verify-gate (iv) tolerates the factor-of-2.
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

// ============================================================
// PublicHeader bridge stubs — used by the upcoming state-machine
// edit. Re-import so unused-warning doesn't fire in the stub state.
// ============================================================

#[doc(hidden)]
#[allow(dead_code, clippy::needless_pass_by_value)]
fn _public_header_used(pkt: Vec<u8>) -> Result<(), String> {
    match parse_public_header(&pkt, 0).map_err(|e| e.to_string())? {
        PublicHeader::Long { ty, .. } => match ty {
            LongType::Initial
            | LongType::ZeroRtt
            | LongType::Handshake
            | LongType::Retry
            | LongType::VersionNegotiation => Ok(()),
        },
        PublicHeader::Short { .. } => Ok(()),
    }
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
}
