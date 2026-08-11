//! QUIC transport layer backed by [`quiche`] over `BoringSSL`.
//!
//! Two layers: the transport-independent typed data model ([`QuicDatagram`], [`QuicStream`]) the
//! rest of the gateway passes around, and a real UDP + TLS 1.3 transport hosted in
//! [`QuicEndpoint`]. [`forward_datagram`] and [`forward_stream`] do **no** network I/O — they are
//! thin synchronous validators guarding the typed model.
//!
//! The migration rationale (quinn 0.11 + rustls/ring → quiche + `BoringSSL`) is in
//! `docs/decisions/quinn-to-quiche-migration.md`. `BoringSSL` links alongside rustls/ring, which is
//! still used on the TLS-over-TCP path.
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

// Always compiled — Mode A passthrough uses these without `quic-terminate`.
pub use lb_security::{RetryTokenSigner, ZeroRttReplayGuard};

// SHARED-1: quiche-free QUIC public-header parser, so Mode A can route by DCID without quiche.
pub mod public_header;

// SHARED-2: UDP datapath trait + tier-3 tokio-UDP impl.
pub mod udp_dataplane;

pub mod passthrough;

pub use passthrough::{PassthroughListener, PassthroughParams};

// ---- termination-only surface (gated behind `quic-terminate`) ----
//
// CF-S15-PASSTHROUGH-FEATURE-GATING: everything below is the H3 termination tree.
// `--no-default-features --features quic-passthrough-only` excludes it all, so
// `cargo bloat --filter quiche` shows ZERO quiche/BoringSSL symbols on the Mode A binary segment.

#[cfg(feature = "quic-terminate")]
use std::time::Duration;

/// Re-exported from `tokio-quiche` so downstream crates stay decoupled from its versioning.
#[cfg(feature = "quic-terminate")]
pub use tokio_quiche::ConnectionParams;

#[cfg(feature = "quic-terminate")]
mod cleanup_guard;
// ROUND8-L7-16: `pub` so the H3 authority-sanitisation invariants can be asserted from a test.
#[cfg(feature = "quic-terminate")]
pub mod conn_actor;
#[cfg(feature = "quic-terminate")]
pub mod h3_bridge;
#[cfg(feature = "quic-terminate")]
pub mod h3_config;
// WS-over-H3 (RFC 9220) — bounded tunnel adapter, shared with `pub mod ws_tunnel` below.
#[cfg(feature = "quic-terminate")]
mod listener;
#[cfg(feature = "quic-terminate")]
pub mod ws_tunnel;
// Mode B (terminate-and-re-originate). Same gate as the H3 surface: it reuses that machinery.
#[cfg(feature = "quic-terminate")]
pub mod raw_proxy;
#[cfg(feature = "quic-terminate")]
mod router;

// PROTO-2-11: exposed so the integration suite can drive the H3 graceful-shutdown helper.
#[cfg(feature = "quic-terminate")]
pub use conn_actor::{H3_INTERNAL_ERROR, H3_NO_ERROR, graceful_h3_shutdown};

// CODE-2-08: re-exported for tests/quic_router_leak.rs.
#[cfg(feature = "quic-terminate")]
pub use cleanup_guard::CidEntryGuard;

#[cfg(feature = "quic-terminate")]
pub use h3_bridge::{H3Request, H3RespEvent, H3RespOut, stream_request_to_h3_upstream};
#[cfg(feature = "quic-terminate")]
pub use listener::{QuicListener, QuicListenerParams};
// Mode B: raw-proxy seam types at the crate root.
#[cfg(feature = "quic-terminate")]
pub use raw_proxy::{
    DGRAM_QUEUE_CAP, MAX_RELAY_STREAMS, RawBackend, RawProxyOutcome, run_raw_proxy_actor,
};
#[cfg(feature = "quic-terminate")]
pub use router::{RouterHandle, RouterParams, spawn as spawn_router};

/// Production ALPN tokens advertised by the H3 listener. `h3-29` is the last pre-RFC draft, still
/// emitted by pinned clients, and is listed SECOND so negotiation prefers the RFC 9114 §3.1 token.
#[cfg(feature = "quic-terminate")]
pub const H3_ALPN_PROTOS: &[&[u8]] = &[b"h3", b"h3-29"];

/// Test-only ALPN for the loopback transport-only rig, which does NOT speak H3 on the wire. Kept
/// under `cfg(test)` so "no production path advertises anything but [`H3_ALPN_PROTOS`]" holds.
#[cfg(all(test, feature = "quic-terminate"))]
pub(crate) const LB_QUIC_TEST_ALPN: &[u8] = b"lb-quic";

/// SNI the loopback client presents. `BoringSSL`'s hostname verifier rejects an iPAddress-type SAN
/// even with a `serverAuth` EKU, so the loopback cert uses a DNS SAN while still targeting
/// 127.0.0.1.
#[cfg(feature = "quic-terminate")]
pub const LB_QUIC_TEST_SNI: &str = "expressgateway.test";

/// Maximum size of one datagram we accept over the UDP socket.
#[cfg(feature = "quic-terminate")]
const MAX_UDP_DATAGRAM_SIZE: usize = 65_535;

/// Budget before the loopback driver treats a test as hung.
#[cfg(feature = "quic-terminate")]
const LOOPBACK_DRIVER_BUDGET: Duration = Duration::from_secs(5);

// CF-S15-PASSTHROUGH-FEATURE-GATING: the whole loopback rig lives in one gated module.
#[cfg(feature = "quic-terminate")]
mod terminate_loopback;

// Re-exported at the crate root so existing import paths keep working.
#[cfg(feature = "quic-terminate")]
pub use terminate_loopback::{
    QuicDatagram, QuicEndpoint, QuicError, QuicStream, forward_datagram, forward_stream,
    roundtrip_datagram, roundtrip_stream,
};
