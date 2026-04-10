//! L4 (transport-layer) load balancing algorithms.
//!
//! All algorithms operate on [`L4Request`]/[`L4Response`] and implement the
//! [`LoadBalancer`] trait.

pub mod consistent_hash;
pub mod least_connection;
pub mod least_load;
pub mod random;
pub mod round_robin;
pub mod weighted_round_robin;

pub use consistent_hash::ConsistentHashBalancer;
pub use least_connection::LeastConnectionBalancer;
pub use least_load::LeastLoadBalancer;
pub use random::RandomBalancer;
pub use round_robin::RoundRobinBalancer;
pub use weighted_round_robin::WeightedRoundRobinBalancer;
