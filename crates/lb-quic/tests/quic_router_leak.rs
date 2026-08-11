//! CODE-2-08 proof — the per-CID DashMap leak on actor panic. The router registers TWO entries
//! per accepted connection; pre-fix, cleanup was two explicit removes AFTER `run_actor().await`,
//! so an unwinding actor left both pinned for the router's lifetime. The fix is a
//! [`CidEntryGuard`] that always removes them on Drop.
//!
//! The workspace release profile is `panic = "abort"`, so a release panic would kill the process
//! before any guard runs; `[profile.test]` keeps the rustc default `unwind` precisely so this
//! test can exercise that path.

use std::sync::Arc;

use dashmap::DashMap;
use lb_quic::CidEntryGuard;

/// Round-4 named invariant: a panicking task that owns a `CidEntryGuard` still removes both
/// DashMap entries during unwind.
#[test]
fn test_panicking_actor_removes_entry() {
    let map: Arc<DashMap<Vec<u8>, ()>> = Arc::new(DashMap::new());
    let router_key = b"router-cid-bytes".to_vec();
    let header_dcid_key = b"header-dcid-bytes".to_vec();

    // A real OS thread rather than the tokio runtime — the guard's drop semantics are
    // executor-agnostic.
    map.insert(router_key.clone(), ());
    map.insert(header_dcid_key.clone(), ());
    assert_eq!(map.len(), 2, "fixture: both entries should be live");

    let map_for_worker = Arc::clone(&map);
    let rk = router_key.clone();
    let hk = header_dcid_key.clone();
    let join = std::thread::spawn(move || {
        let _guard = CidEntryGuard::new(map_for_worker, rk, hk);
        panic!("simulated actor panic — CidEntryGuard must still remove entries");
    });

    // The guard's Drop runs during unwind BEFORE this `join().is_err()` observes the panic.
    let join_result = join.join();
    assert!(
        join_result.is_err(),
        "worker thread did not panic — test fixture is broken"
    );

    assert!(
        !map.contains_key(&router_key),
        "router_key entry leaked after actor panic"
    );
    assert!(
        !map.contains_key(&header_dcid_key),
        "header_dcid_key entry leaked after actor panic"
    );
    assert_eq!(
        map.len(),
        0,
        "DashMap must be empty after the panicked worker's guard drops"
    );
}

/// Sanity counter-test: a clean exit also removes both entries, guarding against a regression
/// where Drop is only wired up on the unwind path.
#[test]
fn clean_exit_also_removes_entries() {
    let map: Arc<DashMap<Vec<u8>, ()>> = Arc::new(DashMap::new());
    let router_key = b"router-clean".to_vec();
    let header_dcid_key = b"header-clean".to_vec();
    map.insert(router_key.clone(), ());
    map.insert(header_dcid_key.clone(), ());

    {
        let _guard = CidEntryGuard::new(
            Arc::clone(&map),
            router_key.clone(),
            header_dcid_key.clone(),
        );
    }

    assert!(!map.contains_key(&router_key));
    assert!(!map.contains_key(&header_dcid_key));
    assert_eq!(map.len(), 0);
}

/// The async-cancel path: dropping the guard inside a future that is itself dropped mid-await
/// must remove the entries — the third Drop trigger after clean-exit and panic-unwind.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancel_drops_entries() {
    let map: Arc<DashMap<Vec<u8>, ()>> = Arc::new(DashMap::new());
    let router_key = b"router-cancel".to_vec();
    let header_dcid_key = b"header-cancel".to_vec();
    map.insert(router_key.clone(), ());
    map.insert(header_dcid_key.clone(), ());

    let map_for_task = Arc::clone(&map);
    let rk = router_key.clone();
    let hk = header_dcid_key.clone();
    let handle = tokio::spawn(async move {
        let _guard = CidEntryGuard::new(map_for_task, rk, hk);
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    });

    tokio::task::yield_now().await;
    handle.abort();
    let _ = handle.await;

    assert_eq!(
        map.len(),
        0,
        "guard must remove entries when its owning future is cancelled"
    );
}
