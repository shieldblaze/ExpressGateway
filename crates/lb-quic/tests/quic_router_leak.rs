//! CODE-2-08 proof — per-CID DashMap leak on actor panic.
//!
//! The router's spawn site at `crates/lb-quic/src/router.rs` registers
//! two DashMap entries per accepted QUIC connection
//! (router_key + header_dcid_key). Pre-CODE-2-08 the cleanup was two
//! explicit `connections.remove(...)` calls AFTER `run_actor().await`
//! — if the actor unwound the `remove` lines never ran and the entries
//! were pinned for the router's lifetime. The fix is a
//! [`CidEntryGuard`] (RAII) that always removes both keys on Drop.
//!
//! ## Profile note
//!
//! The workspace `[profile.release]` is `panic = "abort"` (CODE-2-02),
//! so a release-mode panic kills the process and the guard would never
//! run. `[profile.test]` keeps the rustc default `unwind` precisely so
//! tests like this can exercise the unwind path. The round-4 brief
//! calls this out explicitly: "Under `panic = "abort"` this test must
//! run with `unwind` — use `[profile.test]` to enable unwind, the plan
//! noted this." No special profile flags are needed; the default test
//! build keeps unwind.

use std::sync::Arc;

use dashmap::DashMap;
use lb_quic::CidEntryGuard;

/// Round-4 named invariant: when a task that owns a `CidEntryGuard`
/// panics, the guard's `Drop` runs during unwind and the two DashMap
/// entries are removed.
#[test]
fn test_panicking_actor_removes_entry() {
    let map: Arc<DashMap<Vec<u8>, ()>> = Arc::new(DashMap::new());
    let router_key = b"router-cid-bytes".to_vec();
    let header_dcid_key = b"header-dcid-bytes".to_vec();

    // Simulate the router's spawn site: pre-register both entries, then
    // spawn (here: a real OS thread to avoid the tokio runtime; the
    // guard's drop semantics are agnostic to the executor) a worker
    // that panics with the guard owned in its local scope.
    map.insert(router_key.clone(), ());
    map.insert(header_dcid_key.clone(), ());
    assert_eq!(map.len(), 2, "fixture: both entries should be live");

    let map_for_worker = Arc::clone(&map);
    let rk = router_key.clone();
    let hk = header_dcid_key.clone();
    let join = std::thread::spawn(move || {
        let _guard = CidEntryGuard::new(map_for_worker, rk, hk);
        // Mimic an actor panic — e.g. quiche FFI assertion, hyper
        // overflow, or any of the panic-surface call-sites the round-2
        // review enumerates under §"panic surface".
        panic!("simulated actor panic — CidEntryGuard must still remove entries");
    });

    // The worker thread is expected to panic; join returns Err with
    // the boxed panic payload. The guard's Drop runs during unwind
    // BEFORE this `join().is_err()` observes the panic.
    let join_result = join.join();
    assert!(
        join_result.is_err(),
        "worker thread did not panic — test fixture is broken"
    );

    // Round-4 invariant: both entries are gone.
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

/// Sanity counter-test: a clean exit (no panic) also removes both
/// entries. This guards against a regression where the guard's Drop
/// is somehow only wired up on the unwind path.
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
        // Scope ends → guard drops → both entries removed.
    }

    assert!(!map.contains_key(&router_key));
    assert!(!map.contains_key(&header_dcid_key));
    assert_eq!(map.len(), 0);
}

/// Sanity for the async-cancel path: dropping the guard inside a
/// future that itself is dropped (cancelled mid-await) must remove
/// the entries. This is the third Drop trigger after clean-exit and
/// panic-unwind; covering it shuts the door on "the guard works for
/// panics but not for cancellation" regressions.
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
        // Block forever (virtual time); the abort below cancels us.
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    });

    // Yield once so the task hits the sleep, then abort.
    tokio::task::yield_now().await;
    handle.abort();
    // The aborted future's drop chain runs synchronously inside the
    // executor; we just wait for the handle to settle.
    let _ = handle.await;

    assert_eq!(
        map.len(),
        0,
        "guard must remove entries when its owning future is cancelled"
    );
}
