//! `lb-soak` — ExpressGateway chaos/soak suite.
//!
//! Launches the real `expressgateway` binary as a child, drives co-located load + chaos at it,
//! samples its `/proc` + `/metrics` into a time-series, and computes a BOUNDED/DRIFT verdict
//! (R8). It links NO product crates, so what it measures is what an operator would see.

// Deliberately scoped to the panic-freedom triad — the CI gate's actual intent — not the full
// pedantic set the product crates carry, since lb-soak is a black-box test harness.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// Config/load builders legitimately carry many parameters (cert paths, addrs, caps).
#![allow(clippy::too_many_arguments)]

pub mod backends;
pub mod bench;
pub mod chaos;
pub mod config_gen;
pub mod loadgen;
pub mod metrics;
pub mod procstat;
pub mod timeseries;

pub mod gateway;
pub mod sampler;
