//! File-watching and SIGHUP-triggered hot-reload.
//!
//! [`ConfigWatcher`] monitors a TOML configuration file for changes using the
//! `notify` crate and listens for `SIGHUP` on Unix platforms.  When a change
//! is detected the new file is parsed, validated, and atomically swapped into
//! a shared [`ArcSwap<Config>`](arc_swap::ArcSwap) so readers never see a
//! partially-applied configuration.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::loader;
use crate::model::Config;

/// Watches a configuration file and provides atomic hot-reload.
///
/// Readers obtain the current configuration via [`current`](Self::current),
/// which returns an `Arc<Config>` that is never mutated in place.
pub struct ConfigWatcher {
    /// Atomically-swappable configuration handle.
    config: Arc<ArcSwap<Config>>,
    /// Path to the TOML file being watched.
    path: PathBuf,
}

impl ConfigWatcher {
    /// Create a new watcher with an initial configuration loaded from `path`.
    ///
    /// The file is loaded and validated immediately; an error is returned if
    /// the initial load fails.
    pub fn new(path: &Path) -> Result<Self> {
        let config = loader::load_from_file(path)
            .with_context(|| format!("initial config load from {}", path.display()))?;

        Ok(Self {
            config: Arc::new(ArcSwap::from_pointee(config)),
            path: path.to_path_buf(),
        })
    }

    /// Create a watcher from an already-validated [`Config`].
    ///
    /// `path` is stored so that file-watch reload knows where to read from.
    pub fn from_config(config: Config, path: PathBuf) -> Self {
        Self {
            config: Arc::new(ArcSwap::from_pointee(config)),
            path,
        }
    }

    /// Return an `Arc` snapshot of the current configuration.
    ///
    /// This is wait-free and never blocks.
    pub fn current(&self) -> arc_swap::Guard<Arc<Config>> {
        self.config.load()
    }

    /// Return a clone of the inner [`ArcSwap`] handle so other subsystems
    /// can read the configuration without holding a reference to the watcher.
    pub fn handle(&self) -> Arc<ArcSwap<Config>> {
        Arc::clone(&self.config)
    }

    /// Attempt to reload the configuration from disk.
    ///
    /// The new file is parsed and fully validated **before** the swap is
    /// performed.  If validation fails the existing config is retained and
    /// an error is returned.
    ///
    /// This is an async method that offloads blocking I/O to
    /// `tokio::task::spawn_blocking`.
    pub async fn reload(&self) -> Result<()> {
        info!(path = %self.path.display(), "reloading configuration");

        let new_config = loader::load_from_file_async(&self.path)
            .await
            .context("config reload failed")?;

        let old = self.config.load();
        if **old != new_config {
            self.log_diff(&old, &new_config);
            self.config.store(Arc::new(new_config));
            info!("configuration reloaded successfully");
        } else {
            info!("configuration unchanged, no swap needed");
        }
        Ok(())
    }

    /// Synchronous reload for use outside of async contexts (e.g. tests).
    pub fn reload_sync(&self) -> Result<()> {
        info!(path = %self.path.display(), "reloading configuration (sync)");

        let new_config = loader::load_from_file(&self.path)
            .context("config reload failed")?;

        let old = self.config.load();
        if **old != new_config {
            self.log_diff(&old, &new_config);
            self.config.store(Arc::new(new_config));
            info!("configuration reloaded successfully");
        } else {
            info!("configuration unchanged, no swap needed");
        }
        Ok(())
    }

    /// Log which top-level config sections changed.
    fn log_diff(&self, old: &Config, new: &Config) {
        macro_rules! diff_section {
            ($field:ident) => {
                if old.$field != new.$field {
                    info!(section = stringify!($field), "config section changed");
                }
            };
        }
        diff_section!(global);
        diff_section!(runtime);
        diff_section!(transport);
        diff_section!(buffer);
        diff_section!(tls);
        diff_section!(listeners);
        diff_section!(clusters);
        diff_section!(routes);
        diff_section!(http);
        diff_section!(proxy_protocol);
        diff_section!(health_check);
        diff_section!(circuit_breaker);
        diff_section!(security);
        diff_section!(controlplane);
        diff_section!(graceful_shutdown);
        diff_section!(retry);
    }

    /// Spawn background tasks that trigger [`reload`](Self::reload) on:
    ///
    /// 1. File-system changes detected by `notify`.
    /// 2. `SIGHUP` (Unix only).
    ///
    /// The tasks run until `shutdown` is signalled via the returned
    /// [`tokio::sync::mpsc::Sender`].  Dropping the sender (or sending a
    /// message) shuts everything down gracefully.
    pub fn spawn_watchers(self: &Arc<Self>) -> Result<mpsc::Sender<()>> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);

        // -- File watcher (notify) -------------------------------------------
        let watcher_self = Arc::clone(self);
        let (notify_tx, notify_rx) = mpsc::channel::<()>(16);

        let path_clone = self.path.clone();
        let mut shutdown_file = shutdown_tx.clone().subscribe_to_close(shutdown_rx);
        // `RecommendedWatcher` must be kept alive in the spawned task.
        tokio::spawn(async move {
            // Build a synchronous notify watcher that sends into the async
            // channel.
            let tx = notify_tx;
            let mut watcher: RecommendedWatcher = match notify::recommended_watcher(
                move |res: std::result::Result<Event, notify::Error>| {
                    if let Ok(event) = res
                        && matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
                    {
                        let _ = tx.try_send(());
                    }
                },
            ) {
                Ok(w) => w,
                Err(e) => {
                    error!("failed to create file watcher: {e}");
                    return;
                }
            };

            // Watch the parent directory (some editors do atomic saves by
            // writing a new file and renaming).
            let watch_dir = path_clone
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            if let Err(e) = watcher.watch(watch_dir, RecursiveMode::NonRecursive) {
                error!("failed to start watching {}: {e}", watch_dir.display());
                return;
            }

            info!(path = %watch_dir.display(), "file watcher started");

            // Keep the watcher alive until shutdown.
            shutdown_file.recv().await;
            info!("file watcher shutting down");
        });

        // -- Notify event consumer with debounce -----------------------------
        let reload_self = Arc::clone(&watcher_self);
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            let mut notify_rx = notify_rx;
            loop {
                tokio::select! {
                    msg = notify_rx.recv() => {
                        if msg.is_none() {
                            break;
                        }
                        // Debounce: wait 100ms then drain any queued duplicates.
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        while notify_rx.try_recv().is_ok() {}

                        match reload_self.reload().await {
                            Ok(()) => {}
                            Err(e) => warn!("config reload failed (keeping previous): {e}"),
                        }
                    }
                    _ = shutdown_tx_clone.closed() => {
                        break;
                    }
                }
            }
        });

        // -- SIGHUP handler (Unix only) --------------------------------------
        #[cfg(unix)]
        {
            let sighup_self = Arc::clone(&watcher_self);
            let sighup_shutdown = shutdown_tx.clone();
            tokio::spawn(async move {
                let mut sig =
                    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
                        Ok(s) => s,
                        Err(e) => {
                            error!("failed to register SIGHUP handler: {e}");
                            return;
                        }
                    };

                loop {
                    tokio::select! {
                        result = sig.recv() => {
                            if result.is_none() {
                                break;
                            }
                            info!("received SIGHUP, reloading configuration");
                            match sighup_self.reload().await {
                                Ok(()) => {}
                                Err(e) => warn!("SIGHUP reload failed (keeping previous): {e}"),
                            }
                        }
                        _ = sighup_shutdown.closed() => {
                            info!("SIGHUP handler shutting down");
                            break;
                        }
                    }
                }
            });
        }

        Ok(shutdown_tx)
    }
}

// Helper trait to get a "close" future from the shutdown channel.
// We use the mpsc::Sender's `closed()` method on clones to detect shutdown.
trait SubscribeToClose {
    fn subscribe_to_close(self, rx: mpsc::Receiver<()>) -> mpsc::Receiver<()>;
}

impl SubscribeToClose for mpsc::Sender<()> {
    fn subscribe_to_close(self, rx: mpsc::Receiver<()>) -> mpsc::Receiver<()> {
        rx
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_valid_toml(path: &Path) {
        let mut f = std::fs::File::create(path).unwrap();
        write!(
            f,
            r#"
[global]
environment = "development"
log_level = "debug"
metrics_bind = "127.0.0.1:9090"
"#
        )
        .unwrap();
    }

    fn write_updated_toml(path: &Path) {
        let mut f = std::fs::File::create(path).unwrap();
        write!(
            f,
            r#"
[global]
environment = "production"
log_level = "warn"
metrics_bind = "127.0.0.1:9090"
"#
        )
        .unwrap();
    }

    fn write_invalid_toml(path: &Path) {
        let mut f = std::fs::File::create(path).unwrap();
        write!(
            f,
            r#"
[global]
environment = "staging"
"#
        )
        .unwrap();
    }

    #[test]
    fn watcher_new_loads_and_validates() {
        let dir = std::env::temp_dir().join("eg_watcher_new");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cfg.toml");
        write_valid_toml(&path);

        let watcher = ConfigWatcher::new(&path).expect("should load");
        assert_eq!(
            watcher.current().global.log_level,
            crate::model::LogLevel::Debug
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_swaps_atomically() {
        let dir = std::env::temp_dir().join("eg_watcher_reload");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cfg.toml");
        write_valid_toml(&path);

        let watcher = ConfigWatcher::new(&path).expect("initial load");
        assert_eq!(
            watcher.current().global.log_level,
            crate::model::LogLevel::Debug
        );

        write_updated_toml(&path);
        watcher.reload_sync().expect("reload should succeed");
        assert_eq!(
            watcher.current().global.log_level,
            crate::model::LogLevel::Warn
        );
        assert_eq!(
            watcher.current().global.environment,
            crate::model::Environment::Production
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_keeps_old_config_on_validation_failure() {
        let dir = std::env::temp_dir().join("eg_watcher_fail");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cfg.toml");
        write_valid_toml(&path);

        let watcher = ConfigWatcher::new(&path).expect("initial load");
        let old_env = watcher.current().global.environment;

        write_invalid_toml(&path);
        let result = watcher.reload_sync();
        assert!(result.is_err());
        // Config should still be the original.
        assert_eq!(watcher.current().global.environment, old_env);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_config_works() {
        let cfg = Config::default();
        let watcher = ConfigWatcher::from_config(cfg, PathBuf::from("/tmp/nonexistent.toml"));
        assert_eq!(
            watcher.current().global.environment,
            crate::model::Environment::Production
        );
    }

    #[test]
    fn handle_returns_shared_arc() {
        let cfg = Config::default();
        let watcher = ConfigWatcher::from_config(cfg, PathBuf::from("/tmp/nonexistent.toml"));
        let handle = watcher.handle();
        assert_eq!(
            handle.load().global.environment,
            crate::model::Environment::Production
        );
    }
}
