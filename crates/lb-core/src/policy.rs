//! Load balancing policy enumeration.

use serde::{Deserialize, Serialize};

/// Enumerates all supported load balancing algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LbPolicy {
    /// Simple round-robin: each backend gets one request in turn.
    RoundRobin,
    /// Weighted round-robin: Nginx-style smooth weighted distribution.
    WeightedRoundRobin,
    /// Uniform random selection.
    Random,
    /// Random selection with probability proportional to weight.
    WeightedRandom,
    /// Pick the backend with the fewest active connections.
    LeastConnections,
    /// Pick the backend with the fewest active (in-flight) requests.
    LeastRequest,
    /// Pick two random backends, choose the one with fewer connections.
    PowerOfTwoChoices,
    /// Maglev consistent hashing (Google, 2016).
    Maglev,
    /// Ring-based consistent hashing with virtual nodes.
    RingHash,
    /// Exponentially weighted moving average latency-based routing.
    Ewma,
    /// Hash-based sticky sessions.
    SessionAffinity,
}
