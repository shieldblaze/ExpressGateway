//! Health checking and backend liveness monitoring for ExpressGateway.
//!
//! This crate provides:
//! - **Active health checks**: TCP, UDP, and HTTP probes against backend nodes.
//! - **Health tracking**: Sliding-window success rate with rise/fall thresholds.
//! - **Circuit breaker**: Lock-free state machine to prevent cascading failures.
//! - **Outlier detection**: Passive health monitoring based on real traffic.
//! - **Exponential backoff**: Adaptive retry delays on consecutive failures.
//! - **Caching**: Time-based caching of health check results.
//! - **Scheduler**: Background task for periodic health check execution.

pub mod backoff;
pub mod cache;
pub mod circuit_breaker;
pub mod http;
pub mod outlier;
pub mod scheduler;
pub mod tcp;
pub mod tracker;
pub mod udp;

pub use backoff::ExponentialBackoff;
pub use cache::HealthCache;
pub use circuit_breaker::CircuitBreaker;
pub use http::HttpHealthChecker;
pub use outlier::{OutlierConfig, OutlierDetector};
pub use scheduler::HealthCheckScheduler;
pub use tcp::TcpHealthChecker;
pub use tracker::HealthTracker;
pub use udp::UdpHealthChecker;
