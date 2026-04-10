//! Async runtime abstraction with io_uring support for ExpressGateway.
//!
//! This crate provides a unified async I/O interface that uses io_uring when
//! available (Linux kernel >= 5.1) and falls back to tokio's epoll-based
//! reactor on all other platforms.
//!
//! # Modules
//!
//! - [`backend`] -- Runtime backend detection (io_uring vs epoll).
//! - [`runtime`] -- Unified `Runtime` struct wrapping tokio.
//! - [`listener`] -- TCP listener builder with full socket-option tuning.
//! - [`socket`] -- Socket option definitions and application.
//! - [`backpressure`] -- Write-buffer water-mark flow control.

pub mod backend;
pub mod backpressure;
pub mod listener;
pub mod runtime;
pub mod socket;

// Re-export primary types at crate root for convenience.
pub use backend::RuntimeBackend;
pub use backpressure::BackpressureController;
pub use listener::TcpListenerBuilder;
pub use runtime::Runtime;
pub use socket::SocketOptions;
