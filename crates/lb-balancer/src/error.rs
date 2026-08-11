//! Error types for the lb-balancer crate.

/// Errors that can occur during load balancing decisions.
#[derive(Debug, thiserror::Error)]
pub enum BalancerError {
    /// No backends available to pick from.
    #[error("no backends available")]
    NoBackends,

    /// All backends have zero weight.
    #[error("all backends have zero weight")]
    AllZeroWeight,

    /// The precomputed table is stale — the backend set changed and it must be rebuilt.
    #[error("lookup table is stale; rebuild required")]
    TableStale,

    /// Internal error in the balancer.
    #[error("internal balancer error: {0}")]
    Internal(String),
}
