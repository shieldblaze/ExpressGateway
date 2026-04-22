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
#![warn(clippy::pedantic, clippy::nursery)]
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

/// A lightweight backend representation for load balancing decisions.
///
/// This type is independent of `lb-core` so the balancer crate can be used
/// standalone without pulling in the full core type system.
#[derive(Debug, Clone)]
pub struct Backend {
    /// Unique identifier for this backend.
    pub id: String,
    /// Weight for weighted algorithms (higher = more traffic).
    pub weight: u32,
    /// Current number of active TCP connections.
    pub active_connections: u64,
    /// Current number of active (in-flight) requests.
    pub active_requests: u64,
    /// Exponentially weighted moving average latency in nanoseconds.
    pub latency_ewma_ns: u64,
}

impl Backend {
    /// Create a new backend with default zero state.
    #[must_use]
    pub fn new(id: impl Into<String>, weight: u32) -> Self {
        Self {
            id: id.into(),
            weight,
            active_connections: 0,
            active_requests: 0,
            latency_ewma_ns: 0,
        }
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
