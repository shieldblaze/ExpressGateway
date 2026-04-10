//! L7 (HTTP) load balancing algorithms.
//!
//! All algorithms operate on [`HttpRequest`]/[`HttpResponse`] and implement
//! the [`LoadBalancer`] trait.

pub mod consistent_hash;
pub mod least_response_time;
pub mod random;
pub mod round_robin;

pub use consistent_hash::{HashKeySource, HttpConsistentHashBalancer};
pub use least_response_time::EwmaBalancer;
pub use random::HttpRandomBalancer;
pub use round_robin::HttpRoundRobinBalancer;
