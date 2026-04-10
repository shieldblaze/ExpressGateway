//! Configuration versioning with rollback support.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum number of configuration versions to retain.
pub const MAX_VERSIONS: usize = 100;

/// A snapshot of a configuration version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigVersion {
    /// Monotonically increasing version number.
    pub version: u64,
    /// When this version was created.
    pub created_at: DateTime<Utc>,
    /// Human-readable description of what changed.
    pub description: String,
    /// The serialized configuration snapshot (JSON).
    pub snapshot: serde_json::Value,
}

/// Manages an ordered list of configuration versions.
#[derive(Debug)]
pub struct ConfigVersionStore {
    versions: parking_lot::RwLock<Vec<ConfigVersion>>,
    next_version: parking_lot::Mutex<u64>,
}

impl ConfigVersionStore {
    /// Create a new empty version store.
    pub fn new() -> Self {
        Self {
            versions: parking_lot::RwLock::new(Vec::new()),
            next_version: parking_lot::Mutex::new(1),
        }
    }

    /// Push a new configuration version. Trims old versions if exceeding MAX_VERSIONS.
    pub fn push(&self, description: String, snapshot: serde_json::Value) -> ConfigVersion {
        let mut next = self.next_version.lock();
        let version = *next;
        *next += 1;

        let entry = ConfigVersion {
            version,
            created_at: Utc::now(),
            description,
            snapshot,
        };

        let mut versions = self.versions.write();
        versions.push(entry.clone());

        // Trim old versions
        if versions.len() > MAX_VERSIONS {
            let excess = versions.len() - MAX_VERSIONS;
            versions.drain(..excess);
        }

        entry
    }

    /// Get all stored versions (newest last).
    pub fn list(&self) -> Vec<ConfigVersion> {
        self.versions.read().clone()
    }

    /// Get the current (latest) version, if any.
    pub fn current(&self) -> Option<ConfigVersion> {
        self.versions.read().last().cloned()
    }

    /// Get a specific version by number.
    pub fn get(&self, version: u64) -> Option<ConfigVersion> {
        self.versions
            .read()
            .iter()
            .find(|v| v.version == version)
            .cloned()
    }

    /// Rollback to a specific version. Creates a new version entry with the
    /// snapshot from the target version. Returns the new version entry.
    pub fn rollback(&self, target_version: u64) -> Option<ConfigVersion> {
        let snapshot = {
            let versions = self.versions.read();
            versions
                .iter()
                .find(|v| v.version == target_version)
                .map(|v| v.snapshot.clone())
        };

        snapshot.map(|snap| self.push(format!("Rollback to version {}", target_version), snap))
    }

    /// Get the number of stored versions.
    pub fn len(&self) -> usize {
        self.versions.read().len()
    }

    /// Whether the version store is empty.
    pub fn is_empty(&self) -> bool {
        self.versions.read().is_empty()
    }
}

impl Default for ConfigVersionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_list() {
        let store = ConfigVersionStore::new();
        store.push("initial".into(), serde_json::json!({"key": "v1"}));
        store.push("update".into(), serde_json::json!({"key": "v2"}));

        let versions = store.list();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[1].version, 2);
    }

    #[test]
    fn current_returns_latest() {
        let store = ConfigVersionStore::new();
        assert!(store.current().is_none());

        store.push("v1".into(), serde_json::json!({}));
        store.push("v2".into(), serde_json::json!({"updated": true}));

        let current = store.current().unwrap();
        assert_eq!(current.version, 2);
        assert_eq!(current.description, "v2");
    }

    #[test]
    fn get_specific_version() {
        let store = ConfigVersionStore::new();
        store.push("v1".into(), serde_json::json!({"v": 1}));
        store.push("v2".into(), serde_json::json!({"v": 2}));

        let v1 = store.get(1).unwrap();
        assert_eq!(v1.snapshot, serde_json::json!({"v": 1}));

        let v2 = store.get(2).unwrap();
        assert_eq!(v2.snapshot, serde_json::json!({"v": 2}));

        assert!(store.get(99).is_none());
    }

    #[test]
    fn rollback_creates_new_version() {
        let store = ConfigVersionStore::new();
        store.push("v1".into(), serde_json::json!({"state": "original"}));
        store.push("v2".into(), serde_json::json!({"state": "modified"}));

        let rolled_back = store.rollback(1).unwrap();
        assert_eq!(rolled_back.version, 3);
        assert_eq!(
            rolled_back.snapshot,
            serde_json::json!({"state": "original"})
        );
        assert!(rolled_back.description.contains("Rollback to version 1"));

        assert_eq!(store.len(), 3);
    }

    #[test]
    fn rollback_nonexistent_version() {
        let store = ConfigVersionStore::new();
        assert!(store.rollback(999).is_none());
    }

    #[test]
    fn trims_old_versions() {
        let store = ConfigVersionStore::new();
        for i in 0..110 {
            store.push(format!("v{}", i), serde_json::json!({"i": i}));
        }
        assert_eq!(store.len(), MAX_VERSIONS);

        // The earliest version should be version 11 (first 10 trimmed)
        let versions = store.list();
        assert_eq!(versions[0].version, 11);
    }

    #[test]
    fn len_and_is_empty() {
        let store = ConfigVersionStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.push("v1".into(), serde_json::json!({}));
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
    }
}
