//! Control plane standalone mode tests.

use lb_controlplane::{ConfigManager, FileBackend};

#[test]
fn test_controlplane_standalone_sighup_reload() {
    let dir = std::env::temp_dir().join("eg-test-standalone-sighup");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("config.toml");

    // Write initial config and create manager.
    std::fs::write(&path, "key = \"v1\"").unwrap();
    let backend = FileBackend::new(path.clone());
    let mut mgr = ConfigManager::new(Box::new(backend)).unwrap();
    assert_eq!(mgr.current_config(), "key = \"v1\"");
    assert_eq!(mgr.version(), 1);

    // Reload with no on-disk change -- must return false, version stays at 1.
    let changed = mgr.reload().unwrap();
    assert!(!changed);
    assert_eq!(mgr.version(), 1);

    // Simulate SIGHUP: external process writes a new config to disk.
    std::fs::write(&path, "key = \"v2\"").unwrap();
    let changed = mgr.reload().unwrap();
    assert!(changed, "reload must detect the on-disk change");
    assert_eq!(mgr.current_config(), "key = \"v2\"");
    assert_eq!(mgr.version(), 2);

    // A second reload with no further change returns false again.
    let changed = mgr.reload().unwrap();
    assert!(!changed);
    assert_eq!(mgr.version(), 2);

    let _ = std::fs::remove_dir_all(&dir);
}
