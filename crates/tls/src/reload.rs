//! Certificate hot-reload support.
//!
//! Uses [`ArcSwap`] for atomic `ServerConfig` swaps:
//! - Write operations (reload) swap the config atomically
//! - Read operations (TLS handshakes) get a cheap `Arc` clone
//! - Existing connections continue with their original config
//! - New connections pick up the updated config
//!
//! Integrates with the `notify` file watcher for automatic reloads when
//! certificate or key files change on disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use expressgateway_core::types::{MutualTlsMode, TlsProfile};
use rustls::ServerConfig;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::cert;
use crate::config::TlsConfigBuilder;
use crate::error::{Result, TlsError};

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
    #[inline]
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

    /// Reload from cert and key files on disk.
    ///
    /// Builds a new `ServerConfig` from the given paths and atomically swaps it
    /// in. Returns an error if loading or config construction fails -- the old
    /// config remains active in that case.
    pub fn reload_from_files(
        &self,
        cert_path: &Path,
        key_path: &Path,
        profile: TlsProfile,
        mtls_mode: MutualTlsMode,
        trust_ca: Option<&[u8]>,
    ) -> Result<()> {
        let certs = cert::load_certs(cert_path)?;
        let key = cert::load_private_key(key_path)?;

        let new_config = TlsConfigBuilder::server_config(certs, key, profile, mtls_mode, trust_ca)?;
        self.reload(new_config);

        Ok(())
    }

    /// Spawn a background task that watches cert/key files for changes and
    /// automatically reloads the TLS config.
    ///
    /// The task runs until the returned `JoinHandle` is aborted or the
    /// `shutdown_rx` channel receives a signal. File-system events are
    /// debounced to avoid rapid reloads during atomic writes.
    ///
    /// Returns a shutdown sender: drop it or send `()` to stop the watcher.
    pub fn spawn_file_watcher(
        reloader: Arc<TlsReloader>,
        cert_path: PathBuf,
        key_path: PathBuf,
        profile: TlsProfile,
        mtls_mode: MutualTlsMode,
        trust_ca: Option<Vec<u8>>,
    ) -> std::result::Result<
        (tokio::task::JoinHandle<()>, mpsc::Sender<()>),
        TlsError,
    > {
        use notify::{EventKind, RecursiveMode, Watcher};

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let (fs_tx, mut fs_rx) = mpsc::channel::<()>(16);

        // Set up the file watcher on a background thread (notify uses its own
        // thread internally). We funnel events into a tokio channel.
        let watch_cert = cert_path.clone();
        let watch_key = key_path.clone();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res
                && matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_)
                )
            {
                let _ = fs_tx.try_send(());
            }
        })
        .map_err(|e| TlsError::FileWatcher {
            reason: e.to_string(),
        })?;

        // Watch the parent directories of cert and key files so we catch
        // atomic renames (e.g., certbot writes to a temp file then renames).
        let cert_dir = watch_cert
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let key_dir = watch_key
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        watcher
            .watch(&cert_dir, RecursiveMode::NonRecursive)
            .map_err(|e| TlsError::FileWatcher {
                reason: e.to_string(),
            })?;
        if cert_dir != key_dir {
            watcher
                .watch(&key_dir, RecursiveMode::NonRecursive)
                .map_err(|e| TlsError::FileWatcher {
                    reason: e.to_string(),
                })?;
        }

        let handle = tokio::spawn(async move {
            // Keep the watcher alive for the lifetime of this task.
            let _watcher = watcher;

            loop {
                tokio::select! {
                    Some(()) = fs_rx.recv() => {
                        // Debounce: drain any rapid-fire events and wait a beat.
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        while fs_rx.try_recv().is_ok() {}

                        info!(
                            cert = %cert_path.display(),
                            key = %key_path.display(),
                            "Certificate file change detected, reloading"
                        );

                        let ca_ref = trust_ca.as_deref();
                        match reloader.reload_from_files(
                            &cert_path,
                            &key_path,
                            profile,
                            mtls_mode,
                            ca_ref,
                        ) {
                            Ok(()) => info!("TLS certificate hot-reload succeeded"),
                            Err(e) => {
                                error!(
                                    error = %e,
                                    "TLS certificate hot-reload failed; \
                                     existing config remains active"
                                );
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("TLS file watcher shutting down");
                        break;
                    }
                }
            }
        });

        Ok((handle, shutdown_tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert;

    fn test_cert_path() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/cert.pem"))
    }

    fn test_key_path() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/key.pem"))
    }

    fn make_test_config() -> ServerConfig {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let certs = cert::load_certs(test_cert_path()).expect("load certs");
        let key = cert::load_private_key(test_key_path()).expect("load key");
        TlsConfigBuilder::server_config(
            certs,
            key,
            TlsProfile::Intermediate,
            MutualTlsMode::NotRequired,
            None,
        )
        .expect("build config")
    }

    #[test]
    fn reload_swaps_config() {
        let config1 = make_test_config();
        let reloader = TlsReloader::new(config1);

        let before = reloader.current();
        let config2 = make_test_config();
        reloader.reload(config2);
        let after = reloader.current();

        // The Arc pointers should differ after reload.
        assert!(!Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn reload_from_files_succeeds() {
        let config = make_test_config();
        let reloader = TlsReloader::new(config);

        let result = reloader.reload_from_files(
            test_cert_path(),
            test_key_path(),
            TlsProfile::Intermediate,
            MutualTlsMode::NotRequired,
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn reload_from_nonexistent_files_fails() {
        let config = make_test_config();
        let reloader = TlsReloader::new(config);

        let result = reloader.reload_from_files(
            Path::new("/nonexistent/cert.pem"),
            Path::new("/nonexistent/key.pem"),
            TlsProfile::Intermediate,
            MutualTlsMode::NotRequired,
            None,
        );
        assert!(result.is_err());
    }
}
