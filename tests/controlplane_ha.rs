//! Control plane high-availability mode tests.

use lb_controlplane::{FileBackend, HaPoller};

#[test]
fn test_controlplane_ha_polling() {
    let dir = std::env::temp_dir().join("eg-test-ha-polling");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("ha_config.toml");

    // Write initial config.
    std::fs::write(&path, "version = 1").unwrap();
    let backend = FileBackend::new(path.clone());
    let mut poller = HaPoller::new(Box::new(backend));

    // First poll: returns Some because there is no previous config.
    let result = poller.poll().unwrap();
    assert!(
        result.is_some(),
        "first poll must return the initial config"
    );
    assert_eq!(result.unwrap(), "version = 1");
    assert_eq!(poller.poll_count(), 1);

    // Second poll with no change: returns None.
    let result = poller.poll().unwrap();
    assert!(
        result.is_none(),
        "second poll with no change must return None"
    );
    assert_eq!(poller.poll_count(), 2);

    // External config change on disk.
    std::fs::write(&path, "version = 2").unwrap();
    let result = poller.poll().unwrap();
    assert!(result.is_some(), "poll after disk change must return Some");
    assert_eq!(result.unwrap(), "version = 2");
    assert_eq!(poller.poll_count(), 3);

    // Immediately poll again with no further change.
    let result = poller.poll().unwrap();
    assert!(result.is_none());
    assert_eq!(poller.poll_count(), 4);

    let _ = std::fs::remove_dir_all(&dir);
}
