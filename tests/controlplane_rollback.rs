//! Control plane config rollback tests.

use lb_controlplane::{ConfigManager, FileBackend};

#[test]
fn test_controlplane_rollback_on_invalid() {
    let dir = std::env::temp_dir().join("eg-test-rollback");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("rollback_config.toml");

    // Start with a known good config.
    std::fs::write(&path, "original = true").unwrap();
    let backend = FileBackend::new(path.clone());
    let mut mgr = ConfigManager::new(Box::new(backend)).unwrap();
    assert_eq!(mgr.current_config(), "original = true");
    assert_eq!(mgr.version(), 1);

    // Simulate a config update on disk, then reload.
    std::fs::write(&path, "updated = true").unwrap();
    let changed = mgr.reload().unwrap();
    assert!(changed);
    assert_eq!(mgr.current_config(), "updated = true");
    assert_eq!(mgr.version(), 2);

    // Validate that empty config is rejected.
    assert!(ConfigManager::validate("").is_err());
    assert!(ConfigManager::validate("   ").is_err());

    // Rollback to the original config.
    mgr.rollback("original = true").unwrap();
    assert_eq!(mgr.current_config(), "original = true");
    assert_eq!(mgr.version(), 3);

    // Verify the rollback was persisted to the backend.
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(on_disk, "original = true");

    // Rollback with empty config must fail.
    assert!(mgr.rollback("").is_err());
    // State unchanged after failed rollback.
    assert_eq!(mgr.current_config(), "original = true");
    assert_eq!(mgr.version(), 3);

    let _ = std::fs::remove_dir_all(&dir);
}
