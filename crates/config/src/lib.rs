//! Configuration loading, validation, and hot-reload for ExpressGateway.
//!
//! This crate provides:
//!
//! - **[`model`]** -- all configuration structs with serde support and
//!   production-ready defaults.
//! - **[`loader`]** -- TOML file loading and comprehensive validation.
//! - **[`watcher`]** -- file-system watching (via `notify`) and `SIGHUP`
//!   triggered hot-reload with atomic [`arc_swap`] swaps.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use std::path::Path;
//! use expressgateway_config::{Config, load_from_file};
//!
//! let config = load_from_file(Path::new("config/default.toml")).unwrap();
//! println!("environment: {}", config.global.environment);
//! ```

pub mod loader;
pub mod model;
pub mod watcher;

// Re-export the most commonly used items at crate root for convenience.
pub use loader::{load_from_file, load_from_str, validate};
pub use model::*;
pub use watcher::ConfigWatcher;
