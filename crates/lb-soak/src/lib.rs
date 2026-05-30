//! `lb-soak` — ExpressGateway chaos/soak suite (S20).
//!
//! Permanent test infrastructure: an external-process soak apparatus that
//! launches the real `expressgateway` binary as a child, drives co-located
//! in-process load + chaos at it, and samples the child's `/proc` (RSS / fd /
//! threads) plus its `/metrics` (state-table gauges) into a per-scenario
//! time-series, then computes a BOUNDED/DRIFT stability verdict (R8).
//!
//! The crate links NO product crates — it is a black-box driver against the
//! real binary + its metrics endpoint, so what it measures is what an operator
//! would see. The library half is single-sourced (R12) and unit-tested (R5);
//! the binary half (`eg-soak`) is the scenario orchestrator.
//!
//! Module map:
//! * [`procstat`]   — `/proc/<pid>` RSS/fd/threads (OS half of the signal).
//! * [`metrics`]    — minimal Prometheus text parser (product half).
//! * [`timeseries`] — CSV writer + the BOUNDED/DRIFT trend analyzer.
//! * [`config_gen`] — gateway TOML + cert/key generators per datapath.
//! * [`backends`]   — origin servers (H1 / H2 / QUIC) the gateway proxies to.
//! * [`loadgen`]    — sustained load drivers per datapath.
//! * [`chaos`]      — the chaos injectors.

// Test-harness ergonomics: several config/load builders legitimately carry many
// parameters (cert paths, addrs, caps). Matches the product crates' posture.
#![allow(clippy::too_many_arguments)]

pub mod backends;
pub mod chaos;
pub mod config_gen;
pub mod loadgen;
pub mod metrics;
pub mod procstat;
pub mod timeseries;

pub mod gateway;
pub mod sampler;
