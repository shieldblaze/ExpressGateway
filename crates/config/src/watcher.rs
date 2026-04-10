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
    pub fn reload(&self) -> Result<()> {
        info!(path = %self.path.display(), "reloading configuration");

        let new_config = loader::load_from_file(&self.path).context("config reload failed")?;

        self.config.store(Arc::new(new_config));
        info!("configuration reloaded successfully");
        Ok(())
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
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // -- File watcher (notify) -------------------------------------------
        let watcher_self = Arc::clone(self);
        let (notify_tx, mut notify_rx) = mpsc::channel::<()>(16);

        let path_clone = self.path.clone();
        let notify_tx_clone = notify_tx.clone();
        // `RecommendedWatcher` must be kept alive in the spawned task.
        tokio::spawn(async move {
            // Build a synchronous notify watcher that sends into the async
            // channel.
            let tx = notify_tx_clone;
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
            let watch_path = path_clone.parent().unwrap_or_else(|| Path::new("."));
            if let Err(e) = watcher.watch(watch_path, RecursiveMode::NonRecursive) {
                error!("failed to start watching {}: {e}", watch_path.display());
                return;
            }

            info!(path = %watch_path.display(), "file watcher started");

            // Keep the watcher alive until shutdown.
            shutdown_rx.recv().await;
            info!("file watcher shutting down");
        });

        // -- Notify event consumer -------------------------------------------
        let reload_self = Arc::clone(&watcher_self);
        tokio::spawn(async move {
            while notify_rx.recv().await.is_some() {
                // Debounce: drain any queued duplicate events.
                while notify_rx.try_recv().is_ok() {}

                match reload_self.reload() {
                    Ok(()) => {}
                    Err(e) => warn!("config reload failed (keeping previous): {e}"),
                }
            }
        });

        // -- SIGHUP handler (Unix only) --------------------------------------
        #[cfg(unix)]
        {
            let sighup_self = Arc::clone(&watcher_self);
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
                    sig.recv().await;
                    info!("received SIGHUP, reloading configuration");
                    match sighup_self.reload() {
                        Ok(()) => {}
                        Err(e) => warn!("SIGHUP reload failed (keeping previous): {e}"),
                    }
                }
            });
        }

        Ok(shutdown_tx)
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
        assert_eq!(watcher.current().global.log_level, "debug");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_swaps_atomically() {
        let dir = std::env::temp_dir().join("eg_watcher_reload");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cfg.toml");
        write_valid_toml(&path);

        let watcher = ConfigWatcher::new(&path).expect("initial load");
        assert_eq!(watcher.current().global.log_level, "debug");

        write_updated_toml(&path);
        watcher.reload().expect("reload should succeed");
        assert_eq!(watcher.current().global.log_level, "warn");
        assert_eq!(watcher.current().global.environment, "production");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_keeps_old_config_on_validation_failure() {
        let dir = std::env::temp_dir().join("eg_watcher_fail");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cfg.toml");
        write_valid_toml(&path);

        let watcher = ConfigWatcher::new(&path).expect("initial load");
        let old_env = watcher.current().global.environment.clone();

        write_invalid_toml(&path);
        let result = watcher.reload();
        assert!(result.is_err());
        // Config should still be the original.
        assert_eq!(watcher.current().global.environment, old_env);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_config_works() {
        let cfg = Config::default();
        let watcher = ConfigWatcher::from_config(cfg, PathBuf::from("/tmp/nonexistent.toml"));
        assert_eq!(watcher.current().global.environment, "production");
    }

    #[test]
    fn handle_returns_shared_arc() {
        let cfg = Config::default();
        let watcher = ConfigWatcher::from_config(cfg, PathBuf::from("/tmp/nonexistent.toml"));
        let handle = watcher.handle();
        assert_eq!(handle.load().global.environment, "production");
    }
}
