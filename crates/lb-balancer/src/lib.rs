//! Load balancing algorithms: round-robin, weighted, P2C, Maglev, EWMA, and more.
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
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;
pub mod ewma;
pub mod least_connections;
pub mod least_request;
pub mod maglev;
pub mod p2c;
pub mod random;
pub mod ring_hash;
pub mod round_robin;
pub mod session_affinity;
pub mod weighted_random;
pub mod weighted_round_robin;

pub use error::BalancerError;

use std::sync::Arc;

pub use lb_core::BackendState;

/// A backend as the scheduler sees it.
///
/// `lb_core::BackendState` (Arc'd atomics) is CANONICAL; the plain `u64` fields below are a
/// snapshot cache the hot loop reads. They drift unless [`Self::sync_from_state`] republishes the
/// atomic into the cache — which is how the scheduler and the admin endpoint used to report
/// different values.
///
/// KNOWN GAP: `sync_from_state` has NO production caller (only `tests/balancer_counter_sync.rs`),
/// so in the running binary the cache is only ever what the constructor put there.
#[derive(Debug, Clone)]
pub struct Backend {
    /// Unique identifier for this backend.
    pub id: String,
    /// Weight for weighted algorithms (higher = more traffic).
    pub weight: u32,
    /// Cached `state.active_connections()`; stale until [`Self::sync_from_state`] runs.
    pub active_connections: u64,
    /// Cached `state.active_requests()`; same staleness caveat.
    pub active_requests: u64,
    /// EWMA latency in nanoseconds.
    ///
    /// NEVER WRITTEN IN PRODUCTION: nothing outside this crate and `lb-core` assigns to it or
    /// calls `set_latency_ns`, so it is 0 for every backend at runtime. [`ewma::Ewma::pick`] then
    /// takes its cold-start branch for all of them and the score collapses to
    /// `active_connections + 1` — i.e. selecting `LbPolicy::Ewma` silently gives you
    /// least-connections. Feeding this from the response-completion path is unimplemented.
    pub latency_ewma_ns: u64,
    /// Canonical atomic state, shared with the admin/metrics endpoint. `None` is the test-only
    /// path where the snapshot fields are the sole source.
    pub state: Option<Arc<BackendState>>,
}

impl Backend {
    /// Backend with no atomic binding; production uses [`Self::with_state`].
    #[must_use]
    pub fn new(id: impl Into<String>, weight: u32) -> Self {
        Self {
            id: id.into(),
            weight,
            active_connections: 0,
            active_requests: 0,
            latency_ewma_ns: 0,
            state: None,
        }
    }

    /// Bind the atomic `BackendState` so the scheduler and metrics gauge cannot diverge. The
    /// snapshot is pre-seeded, so a backend built mid-traffic has a consistent first pick.
    #[must_use]
    pub fn with_state(id: impl Into<String>, weight: u32, state: Arc<BackendState>) -> Self {
        let active_connections = state.active_connections();
        let active_requests = state.active_requests();
        let latency_ewma_ns = state.latency_ns();
        Self {
            id: id.into(),
            weight,
            active_connections,
            active_requests,
            latency_ewma_ns,
            state: Some(state),
        }
    }

    /// Refresh the cached snapshot from the atomics; `true` if anything changed.
    ///
    /// NO PRODUCTION CALLER — see the note on [`Backend`].
    pub fn sync_from_state(&mut self) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let new_conn = state.active_connections();
        let new_req = state.active_requests();
        let new_lat = state.latency_ns();
        let changed = self.active_connections != new_conn
            || self.active_requests != new_req
            || self.latency_ewma_ns != new_lat;
        self.active_connections = new_conn;
        self.active_requests = new_req;
        self.latency_ewma_ns = new_lat;
        changed
    }
}

/// Trait for load balancers that pick a backend by index from a slice.
pub trait LoadBalancer: Send + Sync {
    /// Pick a backend, returning its index.
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError>;
}

/// Order-sensitive identity hash over a backend slice. Maglev and ring-hash use it to detect a
/// swapped backend set whose COUNT is unchanged, which a length check would miss.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn backend_identity_hash(backends: &[Backend]) -> u64 {
    // Position-dependent rotation is what makes [A, B] differ from [B, A].
    let mut combined: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, backend) in backends.iter().enumerate() {
        let mut h: u64 = 0;
        for byte in backend.id.bytes() {
            h = h
                .wrapping_mul(0x0100_0000_01b3)
                .wrapping_add(u64::from(byte));
        }
        // Mix in position to make the hash order-dependent.
        h = h.wrapping_add(i as u64);
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        // Truncation is intentional — only the low 6 bits drive the rotation.
        combined ^= h.rotate_left((i as u32) % 64);
    }
    combined
}

/// Trait for load balancers that use a key for consistent hashing / affinity.
pub trait KeyedLoadBalancer: Send + Sync {
    /// Pick a backend for `key`, returning its index.
    fn pick_with_key(&self, backends: &[Backend], key: u64) -> Result<usize, BalancerError>;
}
