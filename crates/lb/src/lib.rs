//! Library-side surface of the `lb` crate — a MINIMAL subset of the binary's modules so
//! integration tests can exercise them without recompiling the whole `main.rs` startup graph.
//!
//! Cargo compiles the binary and the library as separate crates, so both `main.rs` and this file
//! declare `mod xdp;` against the same file — there is no runtime duplication. Keep this thin:
//! anything `main.rs` owns exclusively (runtime bring-up, listener-spawn graph, shutdown wiring)
//! MUST stay private to the binary.
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
