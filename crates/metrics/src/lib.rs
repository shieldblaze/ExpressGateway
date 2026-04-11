//! Prometheus metrics, access logging, and metrics HTTP server.
//!
//! This crate provides:
//!
//! - **`registry`** -- all Prometheus metrics (gauges, counters, histograms)
//!   with a global singleton, [`register_all`](registry::register_all), and
//!   [`error_class`](registry::error_class) for categorised error counting.
//! - **`access_log`** -- structured access-log entries emitted via
//!   [`tracing`] with optional 1-in-N sampling.
//! - **`server`** -- an Axum HTTP server exposing `/metrics` and `/healthz`.

pub mod access_log;
pub mod registry;
pub mod server;

// Re-exports for convenience.
pub use access_log::{AccessLogEntry, AccessLogger};
pub use registry::{
    MetricsRegistry, error_class, global_registry, record_error, register_all, try_global_registry,
};
pub use server::{metrics_router, serve_metrics};
