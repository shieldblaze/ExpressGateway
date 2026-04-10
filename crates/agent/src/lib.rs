//! Data plane agent that connects to the ExpressGateway control plane.
//!
//! Provides:
//! - **[`agent`]** -- `DataPlaneAgent` for registration, heartbeating, and
//!   configuration fetching from the control plane.

pub mod agent;

pub use agent::DataPlaneAgent;
