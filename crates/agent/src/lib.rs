//! Data plane agent that connects to the ExpressGateway control plane.
//!
//! Provides:
//! - **[`agent`]** -- `DataPlaneAgent` for registration, heartbeating, and
//!   configuration fetching from the control plane.
//! - **[`error`]** -- Agent-specific error types.

pub mod agent;
pub mod error;

pub use agent::DataPlaneAgent;
pub use error::AgentError;
