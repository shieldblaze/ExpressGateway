//! Prometheus metrics, access logging, and metrics HTTP server.
//!
//! This crate provides:
//!
//! - **`registry`** -- all Prometheus metrics (gauges, counters, histograms)
//!   with a global singleton and [`register_all`](registry::register_all).
//! - **`access_log`** -- JSON structured access-log entries emitted via
//!   [`tracing`].
//! - **`server`** -- an Axum HTTP server exposing `/metrics` in Prometheus
//!   exposition format.

pub mod access_log;
pub mod registry;
pub mod server;

// Re-exports for convenience.
pub use access_log::{AccessLogEntry, AccessLogger};
pub use registry::{MetricsRegistry, global_registry, register_all};
pub use server::{metrics_router, serve_metrics};
