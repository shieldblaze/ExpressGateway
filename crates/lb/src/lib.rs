//! Library-side surface of the `lb` crate.
//!
//! `lb` is primarily the `expressgateway` binary (see `main.rs`).
//! This `lib.rs` exposes a *minimal* subset of the binary's modules
//! so integration tests under `crates/lb/tests/` can exercise them
//! without re-compiling the entire `main.rs` startup graph.
//!
//! Today's exports are scoped to:
//!
//! * [`xdp`] — the XDP attach helper and its
//!   capability-probe (SEC-2-11 fallback policy).
//!
//! ## Why both `main.rs` and `lib.rs` declare `mod xdp;`
//!
//! Cargo compiles the binary and the library as separate crates;
//! each needs its own `mod` declaration. Both `mod xdp;` lines
//! resolve to the same `src/xdp.rs` file. There is no duplication
//! at runtime — the binary uses *its* copy, integration tests use
//! the library copy.
//!
//! Keep this file thin: anything `main.rs` owns exclusively (the
//! Tokio runtime bring-up, the listener-spawn graph, the
//! shutdown wiring) MUST stay private to the binary. Only modules
//! whose contracts are operator-facing and benefit from
//! integration-test coverage belong here.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![allow(clippy::pedantic, clippy::nursery, clippy::too_many_arguments)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod xdp;
