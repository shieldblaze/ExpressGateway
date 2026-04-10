//! Certificate hot-reload support.
//!
//! Uses [`ArcSwap`] for atomic `ServerConfig` swaps:
//! - Write operations (reload) swap the config atomically
//! - Read operations (TLS handshakes) get a cheap `Arc` clone
//! - Existing connections continue with their original config
//! - New connections pick up the updated config

use std::sync::Arc;

use arc_swap::ArcSwap;
use rustls::ServerConfig;
use tracing::info;

/// Holds a hot-reloadable TLS `ServerConfig`.
///
/// The config is stored in an [`ArcSwap`] which provides lock-free reads
/// and atomic swaps. This means:
/// - TLS handshakes never block on reload operations
/// - Reload operations are wait-free (just an atomic pointer swap)
/// - Existing connections keep their original config reference
pub struct TlsReloader {
    server_config: ArcSwap<ServerConfig>,
}

impl TlsReloader {
    /// Create a new reloader with the given initial config.
    pub fn new(config: ServerConfig) -> Self {
        Self {
            server_config: ArcSwap::from_pointee(config),
        }
    }

    /// Get the current `ServerConfig`.
    ///
    /// This is a cheap operation (loads an `Arc` from the `ArcSwap`).
    pub fn current(&self) -> Arc<ServerConfig> {
        self.server_config.load_full()
    }

    /// Atomically replace the `ServerConfig`.
    ///
    /// New TLS handshakes will use the new config immediately.
    /// Existing connections are unaffected.
    pub fn reload(&self, new_config: ServerConfig) {
        self.server_config.store(Arc::new(new_config));
        info!("TLS server configuration reloaded");
    }
}
