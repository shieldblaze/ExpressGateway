//! ExpressGateway core types, traits, and error definitions.
//!
//! This crate defines the foundational abstractions used across all modules:
//! nodes, load balancers, session persistence, health checking, connections,
//! and listeners.

pub mod backlog;
pub mod connection;
pub mod error;
pub mod health;
pub mod lb;
pub mod listener;
pub mod node;
pub mod protocol;
pub mod session;
pub mod types;

pub use connection::{Connection, ConnectionState};
pub use error::{Error, Result};
pub use health::{Health, HealthCheckConfig, HealthCheckResult, HealthChecker, HealthState};
pub use lb::LoadBalancer;
pub use listener::Listener;
pub use node::{Node, NodeImpl, NodeState};
pub use protocol::Protocol;
pub use session::SessionPersistence;
pub use types::*;
