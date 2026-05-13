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

/// A balancer-level backend representation for load balancing decisions.
///
/// CODE-2-14 — Single source of truth for backend counters. Prior to
/// this commit `Backend` held three plain `u64` fields
/// (active_connections / active_requests / latency_ewma_ns) that
/// duplicated the atomic counters on `lb_core::BackendState`. The two
/// sets could (and did) drift: the scheduler picked from the local
/// `u64` and the admin endpoint reported the atomic — observers saw
/// different values during the gap. After this commit:
///
/// * `lb_core::BackendState` (an Arc'd atomic struct) is canonical.
/// * `Backend::state` holds the Arc; clones share the same atomics.
/// * The legacy `u64` fields remain as a SNAPSHOT cache used by the
///   scheduler's hot loop (one atomic-load per pick is acceptable;
///   the cache is the field). [`Self::sync_from_state`] refreshes
///   the cache from the atomic; production call-sites call it before
///   each pick and the scheduler then reads the cached `u64` value.
/// * Tests that previously mutated the `u64` fields directly continue
///   to compile and work — the field stays `pub`. New production
///   code paths should use `BackendState::inc_connections()` (with
///   the AcqRel ordering from CODE-2-04) and then `sync_from_state()`
///   to publish the increment into the scheduler-visible snapshot.
///
/// The race test `tests/balancer_counter_sync.rs::test_no_divergence_under_load`
/// drives concurrent inc/dec on a shared `BackendState` and asserts
/// the snapshot converges to the atomic — proving the two cannot
/// diverge under bounded race.
#[derive(Debug, Clone)]
pub struct Backend {
    /// Unique identifier for this backend.
    pub id: String,
    /// Weight for weighted algorithms (higher = more traffic).
    pub weight: u32,
    /// Cached snapshot of `state.active_connections()`. Scheduler hot
    /// path reads this; call [`Self::sync_from_state`] before pick.
    pub active_connections: u64,
    /// Cached snapshot of `state.active_requests()`.
    pub active_requests: u64,
    /// Exponentially weighted moving average latency in nanoseconds.
    /// EWMA is updated on response completion in lb-l7; today still a
    /// plain `u64`. Promotion to a Wave-2 atomic is tracked under
    /// CODE-2-14.
    pub latency_ewma_ns: u64,
    /// CODE-2-14 canonical atomic state. Production constructs Backend
    /// via [`Self::with_state`] which binds the same Arc the admin /
    /// metrics endpoint reads from. `None` means "legacy / test-only
    /// path"; the snapshot fields are then the sole source.
    pub state: Option<Arc<BackendState>>,
}

impl Backend {
    /// Create a new backend with default zero state and no atomic
    /// binding. Tests use this; production goes through
    /// [`Self::with_state`].
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

    /// CODE-2-14 canonical constructor: bind the per-backend atomic
    /// `BackendState` so the scheduler and metrics gauge cannot
    /// diverge. The snapshot fields are pre-seeded from the atomic
    /// so a backend constructed mid-traffic has a consistent first
    /// pick.
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

    /// Refresh the cached `u64` snapshot from the atomic state. Cheap
    /// — three relaxed-equivalent loads (the underlying atomics use
    /// AcqRel publishes per CODE-2-04 so loads are Acquire-ordered).
    /// Production scheduler call-sites invoke this before each pick.
    ///
    /// Returns `true` if any field's snapshot changed.
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
    /// Pick a backend from `backends`, returning its index.
    ///
    /// # Errors
    ///
    /// Returns `BalancerError` if the backend list is empty or selection fails.
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError>;
}

/// Compute an identity hash over a backend slice.
///
/// This captures both the set of backend IDs and their order. Two slices
/// produce the same hash if and only if they contain the same IDs in the same
/// positions. Used by [`maglev::Maglev`] and [`ring_hash::RingHash`] to detect
/// stale lookup tables when backends are swapped but count stays the same.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn backend_identity_hash(backends: &[Backend]) -> u64 {
    // Order-dependent hash: for each backend, hash its ID and mix with
    // a position-dependent rotation so that [A, B] != [B, A].
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
        // Combine: XOR with rotation to avoid commutativity.
        // Truncation is intentional: we only need the low 6 bits for rotation.
        combined ^= h.rotate_left((i as u32) % 64);
    }
    combined
}

/// Trait for load balancers that use a key for consistent hashing / affinity.
pub trait KeyedLoadBalancer: Send + Sync {
    /// Pick a backend from `backends` using the given key, returning its index.
    ///
    /// # Errors
    ///
    /// Returns `BalancerError` if the backend list is empty or selection fails.
    fn pick_with_key(&self, backends: &[Backend], key: u64) -> Result<usize, BalancerError>;
}
