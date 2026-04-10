//! Control plane for ExpressGateway (gRPC + REST API).
//!
//! Provides:
//! - **[`rest`]** -- Axum-based REST API for managing nodes, clusters, routes,
//!   listeners, health checks, TLS certificates, and configuration.
//! - **[`store`]** -- In-memory store backed by DashMap for concurrent access.
//! - **[`node_registry`]** -- Node lifecycle state machine with heartbeat tracking.
//! - **[`auth`]** -- Session token authentication with per-node rate limiting.
//! - **[`config_version`]** -- Configuration versioning with rollback support.
//! - **[`reconnect_protection`]** -- Token bucket rate limiter against reconnect storms.
//! - **[`grpc`]** -- gRPC service stubs (to be implemented with tonic-build later).

pub mod auth;
pub mod config_version;
pub mod grpc;
pub mod node_registry;
pub mod reconnect_protection;
pub mod rest;
pub mod store;

pub use auth::SessionAuthenticator;
pub use config_version::{ConfigVersion, ConfigVersionStore};
pub use node_registry::{NodeEntry, NodeRegistryState};
pub use reconnect_protection::ReconnectProtection;
pub use rest::router;
pub use store::ControlPlaneStore;
