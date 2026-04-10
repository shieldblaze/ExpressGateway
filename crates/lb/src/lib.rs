//! Load balancing algorithms and backend selection for ExpressGateway.
//!
//! This crate provides L4 and L7 load balancing algorithms, cookie-based
//! sticky session support, and session persistence implementations.
//!
//! # L4 Algorithms
//!
//! - [`l4::RoundRobinBalancer`] — atomic round-robin
//! - [`l4::WeightedRoundRobinBalancer`] — NGINX-style smooth weighted round-robin
//! - [`l4::LeastConnectionBalancer`] — least active connections
//! - [`l4::LeastLoadBalancer`] — least load percentage
//! - [`l4::RandomBalancer`] — random selection
//! - [`l4::ConsistentHashBalancer`] — Murmur3 consistent hash ring
//!
//! # L7 Algorithms
//!
//! - [`l7::HttpRoundRobinBalancer`] — HTTP round-robin
//! - [`l7::HttpConsistentHashBalancer`] — configurable hash key (header or IP)
//! - [`l7::HttpRandomBalancer`] — HTTP random
//! - [`l7::EwmaBalancer`] — EWMA-based least response time
//!
//! # Sticky Sessions
//!
//! - [`sticky::StickySessionBalancer`] — cookie-based session affinity
//!
//! # Session Persistence
//!
//! - [`session::SourceIpPersistence`] — source IP (/24, /48 masked)
//! - [`session::FourTuplePersistence`] — four-tuple connection identity
//! - [`session::ConsistentHashPersistence`] — stateless ring-based persistence

pub mod l4;
pub mod l7;
pub mod session;
pub mod sticky;

// Re-export all algorithms for convenient access.
pub use l4::{
    ConsistentHashBalancer, LeastConnectionBalancer, LeastLoadBalancer, RandomBalancer,
    RoundRobinBalancer, WeightedRoundRobinBalancer,
};
pub use l7::{
    EwmaBalancer, HashKeySource, HttpConsistentHashBalancer, HttpRandomBalancer,
    HttpRoundRobinBalancer,
};
pub use session::{ConsistentHashPersistence, FourTuplePersistence, SourceIpPersistence};
pub use sticky::StickySessionBalancer;
