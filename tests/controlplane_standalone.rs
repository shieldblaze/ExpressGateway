//! Control plane standalone mode tests.

use lb_config::{parse_config, validate_config};
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

/// CODE-2-13 proof — the backend pool's view of configuration
/// reflects an on-disk TOML reload via the FileBackend + ConfigManager
/// pair. This is the end-to-end shape Wave-2 SIGHUP plumbing will
/// hook into: write new TOML, reload, parse the new content, observe
/// the updated backend set.
#[test]
fn test_backend_pool_observes_toml_reload() {
    let dir = std::env::temp_dir().join("eg-test-cp13-backend-reload");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("config.toml");

    let v1 = r#"
[[listeners]]
address = "127.0.0.1:0"
protocol = "tcp"
[[listeners.backends]]
address = "10.0.0.1:80"
weight = 1
"#;
    std::fs::write(&path, v1).expect("write v1");

    let backend = FileBackend::new(path.clone());
    let mut mgr = ConfigManager::new(Box::new(backend)).expect("manager init");
    // Parse the initial config through lb_config and assert the
    // backend set is what we wrote.
    let cfg = parse_config(mgr.current_config()).expect("parse v1");
    validate_config(&cfg).expect("validate v1");
    assert_eq!(cfg.listeners.len(), 1);
    assert_eq!(cfg.listeners[0].backends.len(), 1);
    assert_eq!(cfg.listeners[0].backends[0].address, "10.0.0.1:80");

    // External process writes v2 with a different backend address.
    let v2 = r#"
[[listeners]]
address = "127.0.0.1:0"
protocol = "tcp"
[[listeners.backends]]
address = "10.0.0.2:80"
weight = 1
[[listeners.backends]]
address = "10.0.0.3:80"
weight = 2
"#;
    std::fs::write(&path, v2).expect("write v2");

    let changed = mgr.reload().expect("reload v2");
    assert!(changed, "reload must detect the v2 backend change");
    assert_eq!(mgr.version(), 2);

    // The pool's *configuration view* (what Wave-2 will hand to
    // lb-balancer's picker after the CODE-2-14 refactor) is the
    // parsed-and-validated config — observe the v2 backend set here.
    let cfg2 = parse_config(mgr.current_config()).expect("parse v2");
    validate_config(&cfg2).expect("validate v2");
    assert_eq!(cfg2.listeners[0].backends.len(), 2);
    let addrs: Vec<&str> = cfg2.listeners[0]
        .backends
        .iter()
        .map(|b| b.address.as_str())
        .collect();
    assert_eq!(addrs, ["10.0.0.2:80", "10.0.0.3:80"]);

    let _ = std::fs::remove_dir_all(&dir);
}
