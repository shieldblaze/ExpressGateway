//! Zero-drop reload under load tests.
//!
//! Without a full async runtime and live traffic, we verify the strongest
//! guarantees we can: rapid reload/rollback cycles produce correct, consistent
//! state at every step, and version numbers increase monotonically.

use lb_controlplane::{ConfigManager, FileBackend};

#[test]
fn test_reload_zero_drop_under_load() {
    let dir = std::env::temp_dir().join("eg-test-zero-drop");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("zero_drop_config.toml");

    std::fs::write(&path, "config = \"v1\"").unwrap();
    let backend = FileBackend::new(path.clone());
    let mut mgr = ConfigManager::new(Box::new(backend)).unwrap();
    assert_eq!(mgr.current_config(), "config = \"v1\"");
    assert_eq!(mgr.version(), 1);

    // Simulate rapid config changes (as if under continuous load).
    let mut expected_version: u64 = 1;
    for i in 2..=20 {
        let new_config = format!("config = \"v{i}\"");
        std::fs::write(&path, &new_config).unwrap();

        let changed = mgr.reload().unwrap();
        assert!(changed, "reload must detect change for v{i}");

        expected_version += 1;
        assert_eq!(mgr.version(), expected_version, "version must be monotonic");
        assert_eq!(
            mgr.current_config(),
            new_config,
            "config must reflect latest after reload"
        );
    }

    // Version should be 20 after 19 successful reloads.
    assert_eq!(mgr.version(), 20);

    // Rollback to a previous config; version still increments.
    mgr.rollback("config = \"v1\"").unwrap();
    assert_eq!(mgr.version(), 21);
    assert_eq!(mgr.current_config(), "config = \"v1\"");

    // Reload after rollback sees the rolled-back value (written by rollback).
    let changed = mgr.reload().unwrap();
    assert!(
        !changed,
        "reload after rollback with no further disk change must return false"
    );
    assert_eq!(mgr.version(), 21);

    // One more disk change to confirm the manager is still functional.
    std::fs::write(&path, "config = \"final\"").unwrap();
    let changed = mgr.reload().unwrap();
    assert!(changed);
    assert_eq!(mgr.version(), 22);
    assert_eq!(mgr.current_config(), "config = \"final\"");

    let _ = std::fs::remove_dir_all(&dir);
}
