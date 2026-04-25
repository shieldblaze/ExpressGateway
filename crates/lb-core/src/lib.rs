//! Core types and traits for the load balancer.
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

mod backend;
mod cluster;
mod error;
mod policy;

pub use backend::{Backend, BackendHealth, BackendState};
pub use cluster::Cluster;
pub use error::CoreError;
pub use policy::LbPolicy;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn test_backend_creation() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        let backend = Backend::new("b1".to_string(), addr, 10);
        assert_eq!(backend.id(), "b1");
        assert_eq!(backend.address(), addr);
        assert_eq!(backend.weight(), 10);
    }

    #[test]
    fn test_backend_health_default() {
        assert_eq!(BackendHealth::default(), BackendHealth::Unknown);
    }

    #[test]
    fn test_backend_state_connections() {
        let state = BackendState::new();
        assert_eq!(state.active_connections(), 0);
        state.inc_connections();
        state.inc_connections();
        assert_eq!(state.active_connections(), 2);
        state.dec_connections();
        assert_eq!(state.active_connections(), 1);
    }

    #[test]
    fn test_backend_state_requests() {
        let state = BackendState::new();
        assert_eq!(state.active_requests(), 0);
        state.inc_requests();
        state.inc_requests();
        state.inc_requests();
        assert_eq!(state.active_requests(), 3);
        state.dec_requests();
        assert_eq!(state.active_requests(), 2);
    }

    #[test]
    fn test_backend_state_latency() {
        let state = BackendState::new();
        assert_eq!(state.latency_ns(), 0);
        state.set_latency_ns(5_000_000);
        assert_eq!(state.latency_ns(), 5_000_000);
    }

    #[test]
    fn test_lb_policy_variants() {
        let policies = [
            LbPolicy::RoundRobin,
            LbPolicy::WeightedRoundRobin,
            LbPolicy::Random,
            LbPolicy::WeightedRandom,
            LbPolicy::LeastConnections,
            LbPolicy::LeastRequest,
            LbPolicy::PowerOfTwoChoices,
            LbPolicy::Maglev,
            LbPolicy::RingHash,
            LbPolicy::Ewma,
            LbPolicy::SessionAffinity,
        ];
        assert_eq!(policies.len(), 11);
    }

    #[test]
    fn test_cluster_creation() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 80);
        let backends = vec![Backend::new("b1".to_string(), addr, 1)];
        let cluster = Cluster::new("web".to_string(), backends, LbPolicy::RoundRobin);
        assert_eq!(cluster.name(), "web");
        assert_eq!(cluster.backends().len(), 1);
        assert_eq!(cluster.policy(), &LbPolicy::RoundRobin);
    }

    #[test]
    fn test_backend_serde_roundtrip() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 443);
        let backend = Backend::new("serde-test".to_string(), addr, 5);
        let json = serde_json::to_string(&backend).unwrap();
        let deserialized: Backend = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id(), backend.id());
        assert_eq!(deserialized.address(), backend.address());
        assert_eq!(deserialized.weight(), backend.weight());
    }

    #[test]
    fn test_cluster_serde_roundtrip() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 80);
        let backends = vec![Backend::new("b1".to_string(), addr, 1)];
        let cluster = Cluster::new("test-cluster".to_string(), backends, LbPolicy::Ewma);
        let json = serde_json::to_string(&cluster).unwrap();
        let deserialized: Cluster = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name(), cluster.name());
        assert_eq!(deserialized.backends().len(), 1);
    }

    #[test]
    fn test_core_error_display() {
        let err = CoreError::InvalidWeight {
            backend_id: "b1".to_string(),
            weight: 0,
        };
        let msg = format!("{err}");
        assert!(msg.contains("b1"));
    }
}
