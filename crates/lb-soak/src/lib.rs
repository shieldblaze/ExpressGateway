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

// Panic-freedom (S34): lb-soak was the only crate missing this deny block, so
// the Panic Freedom Audit CI gate (which globs crates/*/src/lib.rs) failed.
// Scoped to the panic-freedom triad — the gate's actual intent — rather than
// the full pedantic set the product crates carry, since lb-soak is a black-box
// test harness (no missing_docs/indexing_slicing churn). Test modules keep
// using unwrap/expect freely via the cfg_attr(test) allow, exactly as the
// product crates do; the eg-soak binary keeps its own allow.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
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
