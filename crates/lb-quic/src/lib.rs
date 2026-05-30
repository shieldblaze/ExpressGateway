//! QUIC transport layer backed by [`quiche`] 0.28 over `BoringSSL`.
//!
//! This crate exposes two layers:
//!
//! 1. The typed data model — [`QuicDatagram`] and [`QuicStream`] — that
//!    the rest of the gateway (L7 pipeline, HTTP/3 codec, observability)
//!    passes around. The shapes are transport-independent and stable
//!    across the Pillar 3a (quinn) → Pillar 3b (quiche) migration.
//! 2. A real UDP + TLS 1.3 transport, hosted inside [`QuicEndpoint`],
//!    with [`roundtrip_datagram`] and [`roundtrip_stream`] exercising
//!    quiche's unreliable-datagram and unidirectional-stream APIs
//!    end-to-end. These are the functions that drive the manifest-locked
//!    `tests/quic_native.rs` coverage.
//!
//! The free functions [`forward_datagram`] and [`forward_stream`] remain
//! as thin synchronous validators: they do **no** network I/O. They
//! guard the typed model against empty payloads and return a clone.
//!
//! ## Stack
//!
//! Pillar 3b.1 migrated this crate from quinn 0.11 + rustls/ring to
//! [quiche] 0.28 + `BoringSSL`, tracking the decision in
//! `docs/decisions/quinn-to-quiche-migration.md`. The rationale is
//! alignment with Cloudflare's production HTTP/3 stack, measurable
//! throughput advantage over quinn, and vendor-scale CVE response via
//! Cloudflare's networking team. `BoringSSL` links alongside rustls/ring
//! (which remains in use on the TLS-over-TCP listener path and by the
//! Step 5b `TicketRotator`) — the same rustls + `BoringSSL` pairing that
//! Pingora ships in production.
//!
//! [`tokio_quiche::ConnectionParams`] is re-exported so callers wiring
//! Pillar 3b.2's listener into `crates/lb/src/main.rs` can reach the
//! shared configuration without depending on tokio-quiche directly.
//!
//! ## Security hardening (Pillar 3b.3a)
//!
//! * The loopback client path now honors `verify_peer(true)` and
//!   loads the trust anchor via
//!   `quiche::Config::load_verify_locations_from_file`; the tests
//!   build a proper self-signed cert with SAN + `serverAuth` EKU so
//!   `BoringSSL`'s hostname verifier accepts it. The Pillar 3b.1
//!   `verify_peer(false)` workaround is gone.
//! * [`QuicEndpoint::with_retry_signer`] installs a
//!   [`lb_security::RetryTokenSigner`]. quiche 0.28 does not expose
//!   `Config::enable_retry(bool)` — the RETRY handshake is driven by
//!   the application via the free function [`quiche::retry`] in a
//!   custom accept loop. The signer is stored on the endpoint as the
//!   public surface for operator secret rotation; Pillar 3b.3c's
//!   custom accept loop consumes it against the on-wire flow per
//!   RFC 9000 §8.1.3.
//! * [`QuicEndpoint::with_replay_filter`] installs a
//!   [`lb_security::ZeroRttReplayGuard`]. The filter exposes
//!   [`check_0rtt_token`](lb_security::ZeroRttReplayGuard::check_0rtt_token),
//!   which the Pillar 3b.3c accept loop will call before handing any
//!   0-RTT early-data bytes to the application.
//!
//! Deferred to later Pillar 3b steps: Alt-Svc injection on H2/H1
//! responses (3b.3b); CID-routed upstream pool + wiring the QUIC
//! listener into `crates/lb/src/main.rs` (3b.3c); `curl --http3` and
//! `h3i` interop (3b.4).
//!
//! [quiche]: https://github.com/cloudflare/quiche
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![allow(clippy::pedantic, clippy::nursery)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::match_wildcard_for_single_variants
    )
)]

// S15 A1/A2 shared surface (always compiled — Mode A passthrough uses
// these alongside the termination tree, and the passthrough-only build
// keeps them).
pub use lb_security::{RetryTokenSigner, ZeroRttReplayGuard};

// SHARED-1 (S15 A1): quiche-free QUIC public-header parser. Mode A
// passthrough routes packets by Connection ID without decrypting; the
// router calls into this module on every inbound datagram. See
// `audit/quic/s15-design.md` §2 for the wire-format spec this parser
// implements and §A1 for the verify-bar. Namespace-explicit by lead
// directive — callers use `lb_quic::public_header::*`, no re-exports.
pub mod public_header;

// SHARED-2 (S15 A2): UDP datapath trait + tier-3 tokio-UDP impl. Shared
// between Mode A (this session) and Mode B (S16); see `s15-design.md`
// §10 for the stable seam contract.
pub mod udp_dataplane;

// S15 A2: Mode A passthrough router (the new datapath this session
// ships). Builds on `public_header` + `udp_dataplane`; no quiche
// dependency on the Mode A code path — see `Cargo.toml` features and
// the `quic-passthrough-only` build for the NEVER-DECRYPTED LINKAGE
// proof.
pub mod passthrough;

// S15 A2-8: re-export the Mode A listener handle + construction params
// so the root binary (crates/lb/src/main.rs) can wire
// `lb_config::PassthroughConfig` → `PassthroughListener::spawn` without
// reaching into `lb_quic::passthrough::*` directly.
pub use passthrough::{PassthroughListener, PassthroughParams};

// ---- termination-only surface (gated behind `quic-terminate`) -------
//
// S15 A2 (a1) — CF-S15-PASSTHROUGH-FEATURE-GATING. Everything below is
// the existing H3 termination router/actor/bridge/listener tree. It
// requires `quiche`, `tokio-quiche`, `lb-io`, `lb-h3`, `lb-core`,
// `hyper`, `http-body-util`. Building with
// `--no-default-features --features quic-passthrough-only` excludes
// all of it, so `cargo bloat --filter quiche` shows ZERO
// quiche::Connection / BoringSSL symbols on the Mode A binary segment
// (owner ruling §9.5 primary item 1).

// Termination-side `std::time::Duration` is needed only by
// `LOOPBACK_DRIVER_BUDGET` below.
#[cfg(feature = "quic-terminate")]
use std::time::Duration;

/// Re-exported from `tokio-quiche`. Pillar 3b.2 wires a listener in the
/// root binary that consumes a [`ConnectionParams`]; lifting the symbol
/// here keeps downstream crates decoupled from `tokio-quiche` versioning.
#[cfg(feature = "quic-terminate")]
pub use tokio_quiche::ConnectionParams;

#[cfg(feature = "quic-terminate")]
mod cleanup_guard;
// ROUND8-L7-16: `conn_actor` is now `pub` so the H3 authority-
// enforcement proof (`tests/round8_h3_authority_enforced.rs`) can
// drive the REAL `run_actor` / `ActorParams` / `InboundPacket`
// against a real accept-counting probe backend — the same proof
// shape the H1/H2 L7-09 tests use. `h3_bridge` is already `pub mod`
// for the same reason.
#[cfg(feature = "quic-terminate")]
pub mod conn_actor;
#[cfg(feature = "quic-terminate")]
pub mod h3_bridge;
#[cfg(feature = "quic-terminate")]
mod listener;
// SESSION 16 / Mode B (terminate-and-re-originate). Same gate as the H3
// termination tree: it reuses the client-facing termination machinery
// (`conn_actor::ActorParams`/`InboundPacket`) and the `lb-io` QUIC dial
// pool (`DedicatedQuic`), all of which require `quiche` + `lb-io` —
// i.e. the `quic-terminate` deps. `pub` so the verifier's wire test +
// the B6 `lb/src/main.rs` wiring can construct `RawBackend` and (via the
// test hook) drive `run_raw_proxy_actor`. See `audit/quic/s16-plan.md`.
#[cfg(feature = "quic-terminate")]
pub mod raw_proxy;
#[cfg(feature = "quic-terminate")]
mod router;

// PROTO-2-11: expose the H3 graceful-shutdown helper so the integration
// test in `tests/h3_graceful_close.rs` can drive it standalone without
// having to spin up a full `ConnectionActor` (which would otherwise
// require pulling the H3 bridge, TCP pool, and backend wiring through
// the test rig).
#[cfg(feature = "quic-terminate")]
pub use conn_actor::{H3_INTERNAL_ERROR, H3_NO_ERROR, graceful_h3_shutdown};

// CODE-2-08: re-exported so tests/quic_router_leak.rs can call
// `CidEntryGuard::new(...)` from the integration-test target.
#[cfg(feature = "quic-terminate")]
pub use cleanup_guard::CidEntryGuard;

#[cfg(feature = "quic-terminate")]
pub use h3_bridge::{
    H3Request, H3RespEvent, H3RespOut, H3UpstreamResponse, request_h3_upstream,
    stream_request_to_h3_upstream,
};
#[cfg(feature = "quic-terminate")]
pub use listener::{QuicListener, QuicListenerParams};
// SESSION 16 / Mode B: re-export the raw-proxy seam types at the crate
// root so the verifier's wire test + the B6 binary wiring import them
// without reaching into `lb_quic::raw_proxy::*`.
#[cfg(feature = "quic-terminate")]
pub use raw_proxy::{RawBackend, RawProxyOutcome, run_raw_proxy_actor};
#[cfg(feature = "quic-terminate")]
pub use router::{RouterHandle, RouterParams, spawn as spawn_router};

/// Production ALPN tokens advertised by the H3 listener.
///
/// * `h3` is the RFC 9114 §3.1 IANA-registered identifier — mandatory
///   for any peer claiming HTTP/3.
/// * `h3-29` is the last pre-RFC IETF draft and is still emitted by
///   clients pinned to draft-29 (chromium < 91, quic-go < 0.31). Listed
///   second so negotiation prefers the RFC token whenever the client
///   advertises both.
///
/// quiche 0.28 passes the ALPN list straight through to BoringSSL's
/// `SSL_CTX_set_alpn_protos`; both tokens are emitted verbatim in the
/// TLS 1.3 ClientHello / EncryptedExtensions exchange (PROTO-2-02).
#[cfg(feature = "quic-terminate")]
pub const H3_ALPN_PROTOS: &[&[u8]] = &[b"h3", b"h3-29"];

/// Test-only ALPN for the loopback transport-only rig that does **not**
/// speak H3 over the wire. Never advertised from a production listener
/// — the [`build_config`] helper always installs [`H3_ALPN_PROTOS`].
///
/// Pre-PROTO-2-02 this constant was exported as `LB_QUIC_ALPN` and
/// installed on the production server config. Round 4 moved it under
/// `#[cfg(test)]` so the audit invariant "no production code path
/// advertises anything other than `H3_ALPN_PROTOS`" holds.
#[cfg(all(test, feature = "quic-terminate"))]
pub(crate) const LB_QUIC_TEST_ALPN: &[u8] = b"lb-quic";

/// SNI the loopback client presents.
///
/// Pillar 3b.3a turned client peer-cert verification back on;
/// `BoringSSL`'s hostname verifier rejects an iPAddress-type SAN even
/// with a `serverAuth` EKU, so the loopback test cert uses a DNS SAN
/// of `expressgateway.test` and the client sends that name as SNI
/// while still targeting `SocketAddr(127.0.0.1, <port>)`. Pillar
/// 3b.3c will accept an SNI override via the endpoint builder when
/// the real listener lands; for now this constant is the default.
#[cfg(feature = "quic-terminate")]
pub const LB_QUIC_TEST_SNI: &str = "expressgateway.test";

/// Maximum size of one datagram we accept over the UDP socket.
#[cfg(feature = "quic-terminate")]
const MAX_UDP_DATAGRAM_SIZE: usize = 65_535;

/// Budget for how long the loopback driver will keep spinning before
/// treating a test as hung. Loopback handshake + one-shot roundtrip
/// completes well under 200 ms on idle hardware.
#[cfg(feature = "quic-terminate")]
const LOOPBACK_DRIVER_BUDGET: Duration = Duration::from_secs(5);

// S15 A2 (a1) — CF-S15-PASSTHROUGH-FEATURE-GATING. Move the loopback
// rig + QuicError / QuicDatagram / QuicStream / QuicEndpoint /
// build_config / random_scid / drive helpers into a single
// terminate_loopback.rs module gated behind `quic-terminate`. The
// `--no-default-features --features quic-passthrough-only` build
// excludes the whole module so `cargo bloat --filter quiche` shows
// ZERO quiche::Connection / BoringSSL symbols on the Mode A binary.
#[cfg(feature = "quic-terminate")]
mod terminate_loopback;

// Re-export the loopback-rig public surface at the crate root so
// existing termination-side callers (the lb binary, integration
// tests) keep their import paths.
#[cfg(feature = "quic-terminate")]
pub use terminate_loopback::{
    QuicDatagram, QuicEndpoint, QuicError, QuicStream, forward_datagram, forward_stream,
    roundtrip_datagram, roundtrip_stream,
};
