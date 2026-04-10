//! Control plane for ExpressGateway (gRPC + REST API).
//!
//! Provides:
//! - **[`rest`]** -- Axum-based REST API for managing nodes, clusters, routes,
//!   listeners, health checks, TLS certificates, traffic policies, and configuration.
//! - **[`store`]** -- In-memory store backed by DashMap for concurrent access.
//! - **[`node_registry`]** -- Node lifecycle state machine with heartbeat tracking.
//! - **[`auth`]** -- Session token authentication with per-node rate limiting.
//! - **[`config_version`]** -- Configuration versioning with rollback support.
//! - **[`reconnect_protection`]** -- Token bucket rate limiter against reconnect storms.
//! - **[`grpc`]** -- gRPC service implementations for agent communication.
//! - **[`ha`]** -- High-availability traits (ConfigStore, ServiceDiscovery, LeaderElection)
//!   with in-memory, etcd, Consul, and ZooKeeper backends.
//! - **[`error`]** -- Unified error types with HTTP status code mapping.

pub mod auth;
pub mod config_version;
pub mod error;
pub mod grpc;
pub mod ha;
pub mod node_registry;
pub mod reconnect_protection;
pub mod rest;
pub mod store;

pub use auth::SessionAuthenticator;
pub use config_version::{ConfigVersion, ConfigVersionStore};
pub use error::ControlPlaneError;
pub use grpc::GrpcServices;
pub use ha::{ConfigStore, LeaderElection, ServiceDiscovery};
pub use node_registry::{NodeEntry, NodeRegistryState};
pub use reconnect_protection::ReconnectProtection;
pub use rest::router;
pub use store::ControlPlaneStore;
