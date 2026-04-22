//! Control plane for configuration distribution and cluster management.
//!
//! Provides a `ConfigBackend` trait for pluggable storage, a file-backed
//! implementation, an in-memory implementation for testing, and a
//! `ConfigManager` for standalone SIGHUP-style reload with rollback.
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
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::PathBuf;
use std::sync::Mutex;

/// Errors from the control plane.
#[derive(Debug, thiserror::Error)]
pub enum ControlPlaneError {
    /// I/O error reading or writing config.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Config validation failed.
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// Backend storage error.
    #[error("backend error: {0}")]
    Backend(String),

    /// Lock poisoned (only possible via `std::sync::Mutex`).
    #[error("lock poisoned")]
    LockPoisoned,
}

/// Trait for configuration storage backends.
pub trait ConfigBackend: Send + Sync + std::fmt::Debug {
    /// Load the current configuration as a string.
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError` if the read fails.
    fn load(&self) -> Result<String, ControlPlaneError>;

    /// Store a configuration string.
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError` if the write fails.
    fn store(&self, config: &str) -> Result<(), ControlPlaneError>;
}

/// File-backed configuration storage.
///
/// Writes are atomic: data is written to a temporary file in the same directory,
/// then renamed into place. This ensures the config file is never left in a
/// partial state after a crash.
#[derive(Debug)]
pub struct FileBackend {
    path: PathBuf,
}

impl FileBackend {
    /// Create a `FileBackend` pointing at the given path.
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl ConfigBackend for FileBackend {
    fn load(&self) -> Result<String, ControlPlaneError> {
        std::fs::read_to_string(&self.path).map_err(ControlPlaneError::Io)
    }

    fn store(&self, config: &str) -> Result<(), ControlPlaneError> {
        let dir = self
            .path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let tmp_name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or_else(|| ".config.tmp".to_owned(), |n| format!(".{n}.tmp"));
        let tmp_path = dir.join(tmp_name);
        std::fs::write(&tmp_path, config).map_err(ControlPlaneError::Io)?;
        std::fs::rename(&tmp_path, &self.path).map_err(ControlPlaneError::Io)?;
        Ok(())
    }
}

/// In-memory configuration backend, primarily for testing.
#[derive(Debug)]
pub struct InMemoryBackend {
    data: Mutex<String>,
}

impl InMemoryBackend {
    /// Create a new in-memory backend with the given initial config.
    #[must_use]
    pub fn new(initial: &str) -> Self {
        Self {
            data: Mutex::new(initial.to_owned()),
        }
    }

    /// Update the stored config (simulates an external write).
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError::LockPoisoned` if the lock is poisoned.
    pub fn set(&self, config: &str) -> Result<(), ControlPlaneError> {
        let mut guard = self
            .data
            .lock()
            .map_err(|_| ControlPlaneError::LockPoisoned)?;
        config.clone_into(&mut guard);
        drop(guard);
        Ok(())
    }
}

impl ConfigBackend for InMemoryBackend {
    fn load(&self) -> Result<String, ControlPlaneError> {
        let guard = self
            .data
            .lock()
            .map_err(|_| ControlPlaneError::LockPoisoned)?;
        Ok(guard.clone())
    }

    fn store(&self, config: &str) -> Result<(), ControlPlaneError> {
        let mut guard = self
            .data
            .lock()
            .map_err(|_| ControlPlaneError::LockPoisoned)?;
        config.clone_into(&mut guard);
        drop(guard);
        Ok(())
    }
}

/// Configuration manager for standalone mode (SIGHUP reload).
///
/// Tracks the current config string, a monotonic version counter,
/// and supports reload, validation, and rollback (both explicit and
/// automatic to the previous config).
#[derive(Debug)]
pub struct ConfigManager {
    backend: Box<dyn ConfigBackend>,
    current_config: String,
    version: u64,
    previous_config: Option<String>,
}

impl ConfigManager {
    /// Load the initial config from the backend and create a manager.
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError` if the initial load fails.
    pub fn new(backend: Box<dyn ConfigBackend>) -> Result<Self, ControlPlaneError> {
        let config = backend.load()?;
        Self::validate(&config)?;
        Ok(Self {
            backend,
            current_config: config,
            version: 1,
            previous_config: None,
        })
    }

    /// Reload the config from the backend.
    ///
    /// Returns `true` if the config changed, `false` otherwise.
    /// On a successful change, the old config is saved for rollback via
    /// [`rollback_to_previous`](Self::rollback_to_previous).
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError` if the load or validation fails.
    pub fn reload(&mut self) -> Result<bool, ControlPlaneError> {
        let new_config = self.backend.load()?;
        if new_config == self.current_config {
            return Ok(false);
        }
        Self::validate(&new_config)?;
        let old = std::mem::replace(&mut self.current_config, new_config);
        self.previous_config = Some(old);
        self.version += 1;
        Ok(true)
    }

    /// The current configuration string.
    #[must_use]
    pub fn current_config(&self) -> &str {
        &self.current_config
    }

    /// The current monotonic version counter.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// The previous configuration, if a reload has occurred.
    #[must_use]
    pub fn previous_config(&self) -> Option<&str> {
        self.previous_config.as_deref()
    }

    /// Validate a configuration string.
    ///
    /// The config must be non-empty and must parse as valid TOML.
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError::InvalidConfig` if the config is empty or
    /// is not valid TOML.
    pub fn validate(config: &str) -> Result<(), ControlPlaneError> {
        if config.trim().is_empty() {
            return Err(ControlPlaneError::InvalidConfig(
                "config must not be empty".into(),
            ));
        }
        config
            .parse::<toml::Value>()
            .map_err(|e| ControlPlaneError::InvalidConfig(format!("invalid TOML: {e}")))?;
        Ok(())
    }

    /// Rollback: store the given config to the backend and reload.
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError` if the store or reload fails.
    pub fn rollback(&mut self, previous: &str) -> Result<(), ControlPlaneError> {
        Self::validate(previous)?;
        self.backend.store(previous)?;
        let old = std::mem::replace(&mut self.current_config, previous.to_owned());
        self.previous_config = Some(old);
        self.version += 1;
        Ok(())
    }

    /// Rollback to the previously loaded config (saved automatically during
    /// [`reload`](Self::reload)).
    ///
    /// Returns `Ok(true)` if the rollback was applied, `Ok(false)` if there
    /// is no previous config to roll back to.
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError` if the store fails.
    pub fn rollback_to_previous(&mut self) -> Result<bool, ControlPlaneError> {
        let Some(prev) = self.previous_config.take() else {
            return Ok(false);
        };
        self.backend.store(&prev)?;
        let old = std::mem::replace(&mut self.current_config, prev);
        self.previous_config = Some(old);
        self.version += 1;
        Ok(true)
    }
}

/// Simplified HA poller that checks for config changes.
#[derive(Debug)]
pub struct HaPoller {
    primary: Box<dyn ConfigBackend>,
    last_config: Option<String>,
    poll_count: u64,
}

impl HaPoller {
    /// Create a new HA poller backed by the given primary backend.
    #[must_use]
    pub fn new(primary: Box<dyn ConfigBackend>) -> Self {
        Self {
            primary,
            last_config: None,
            poll_count: 0,
        }
    }

    /// Poll the primary for a config change.
    ///
    /// Returns `Some(new_config)` if the config changed since the last poll,
    /// or `None` if unchanged.
    ///
    /// # Errors
    ///
    /// Returns `ControlPlaneError` if the load fails.
    pub fn poll(&mut self) -> Result<Option<String>, ControlPlaneError> {
        self.poll_count += 1;
        let config = self.primary.load()?;

        match &self.last_config {
            Some(last) if *last == config => Ok(None),
            _ => {
                self.last_config = Some(config.clone());
                Ok(Some(config))
            }
        }
    }

    /// Number of times `poll` has been called.
    #[must_use]
    pub const fn poll_count(&self) -> u64 {
        self.poll_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_backend_load_store() {
        let backend = InMemoryBackend::new("hello");
        assert_eq!(backend.load().unwrap(), "hello");

        backend.store("world").unwrap();
        assert_eq!(backend.load().unwrap(), "world");
    }

    #[test]
    fn config_manager_initial_load() {
        let backend = InMemoryBackend::new("key = \"value\"");
        let mgr = ConfigManager::new(Box::new(backend)).unwrap();
        assert_eq!(mgr.current_config(), "key = \"value\"");
        assert_eq!(mgr.version(), 1);
        assert!(mgr.previous_config().is_none());
    }

    #[test]
    fn config_manager_reload_no_change() {
        let backend = InMemoryBackend::new("key = \"value\"");
        let mut mgr = ConfigManager::new(Box::new(backend)).unwrap();
        assert!(!mgr.reload().unwrap());
        assert_eq!(mgr.version(), 1);
    }

    #[test]
    fn config_manager_reload_saves_previous() {
        let backend = InMemoryBackend::new("key = \"v1\"");
        let mut mgr = ConfigManager::new(Box::new(backend)).unwrap();

        // Simulate an external config change via the backend.
        mgr.backend.store("key = \"v2\"").unwrap();
        assert!(mgr.reload().unwrap());
        assert_eq!(mgr.current_config(), "key = \"v2\"");
        assert_eq!(mgr.previous_config(), Some("key = \"v1\""));
        assert_eq!(mgr.version(), 2);
    }

    #[test]
    fn config_manager_rollback_to_previous() {
        let backend = InMemoryBackend::new("key = \"v1\"");
        let mut mgr = ConfigManager::new(Box::new(backend)).unwrap();

        mgr.backend.store("key = \"v2\"").unwrap();
        assert!(mgr.reload().unwrap());
        assert_eq!(mgr.current_config(), "key = \"v2\"");

        // Rollback to v1.
        assert!(mgr.rollback_to_previous().unwrap());
        assert_eq!(mgr.current_config(), "key = \"v1\"");
        assert_eq!(mgr.version(), 3);
    }

    #[test]
    fn config_manager_rollback_to_previous_none() {
        let backend = InMemoryBackend::new("key = \"v1\"");
        let mut mgr = ConfigManager::new(Box::new(backend)).unwrap();

        // No previous config yet.
        assert!(!mgr.rollback_to_previous().unwrap());
    }

    #[test]
    fn validate_empty_is_error() {
        assert!(ConfigManager::validate("").is_err());
        assert!(ConfigManager::validate("   ").is_err());
    }

    #[test]
    fn validate_non_empty_ok() {
        assert!(ConfigManager::validate("key = \"value\"").is_ok());
    }

    #[test]
    fn validate_invalid_toml_is_error() {
        assert!(ConfigManager::validate("not valid {{{{ toml").is_err());
    }

    #[test]
    fn validate_valid_toml_ok() {
        let config = r#"
[server]
port = 8080
host = "localhost"
"#;
        assert!(ConfigManager::validate(config).is_ok());
    }

    #[test]
    fn file_backend_atomic_write() {
        let dir = std::env::temp_dir().join("lb-controlplane-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");

        let backend = FileBackend::new(path);
        backend.store("key = \"value\"").unwrap();

        let loaded = backend.load().unwrap();
        assert_eq!(loaded, "key = \"value\"");

        // Verify the temp file was cleaned up (rename removes it).
        let tmp_path = dir.join(".test-config.toml.tmp");
        assert!(!tmp_path.exists());

        // Clean up.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_manager_rejects_invalid_initial_config() {
        let backend = InMemoryBackend::new("not valid {{{{ toml");
        let err = ConfigManager::new(Box::new(backend)).unwrap_err();
        assert!(
            matches!(err, ControlPlaneError::InvalidConfig(_)),
            "expected InvalidConfig, got: {err}"
        );
    }

    #[test]
    fn config_manager_rejects_empty_initial_config() {
        let backend = InMemoryBackend::new("");
        let err = ConfigManager::new(Box::new(backend)).unwrap_err();
        assert!(matches!(err, ControlPlaneError::InvalidConfig(_)));
    }

    #[test]
    fn ha_poller_first_poll_returns_some() {
        let backend = InMemoryBackend::new("key = \"v1\"");
        let mut poller = HaPoller::new(Box::new(backend));
        let result = poller.poll().unwrap();
        assert!(result.is_some());
        assert_eq!(poller.poll_count(), 1);
    }

    #[test]
    fn ha_poller_no_change_returns_none() {
        let backend = InMemoryBackend::new("key = \"v1\"");
        let mut poller = HaPoller::new(Box::new(backend));
        let _ = poller.poll().unwrap();
        let result = poller.poll().unwrap();
        assert!(result.is_none());
        assert_eq!(poller.poll_count(), 2);
    }
}
