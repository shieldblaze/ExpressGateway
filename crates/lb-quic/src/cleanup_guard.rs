//! CODE-2-08 — RAII drop-guard that removes per-CID DashMap entries
//! unconditionally on actor exit (clean / cancelled / panicked).
//!
//! Before this guard, the spawn site in [`crate::router`] removed
//! entries with two explicit `connections.remove(...)` calls *after*
//! `run_actor(actor).await`. If `run_actor` panics (unwind), the
//! `await` returns mid-unwind and the cleanup calls never run. The
//! `mpsc::Sender` is dropped (so a subsequent packet on the dead CID
//! does see `Closed` and reaps via the existing
//! [`crate::router::forward_to_actor`] path), but a CID that NEVER
//! sees a second packet — idle, NAT-rebind, attacker-injected DCID —
//! is pinned for the router's lifetime. Bound on the leak is
//! `2 * max_connections` because each accepted connection registers
//! two entries (router_key + header_dcid_key); after that the
//! cap-check in [`crate::router`] refuses new connections, turning
//! the leak into a **denial-of-service via panic exhaustion**.
//!
//! Under `panic = "abort"` (CODE-2-02) the release process dies on
//! panic so the guard is dead code there. The guard exists for
//! dev/test (CODE-2-11 proptest / loom) where `unwind` is preserved
//! and the guarantee matters.

use std::hash::Hash;
use std::sync::Arc;

use dashmap::DashMap;

/// RAII removal of up to two DashMap entries. Dropping the guard —
/// on normal scope exit, async-cancel future-drop, or panic unwind —
/// removes both entries.
///
/// The keys are stored as `Option<K>` so [`Self::disarm`] can `take`
/// them to make the guard a no-op (used for the happy path where the
/// caller wants to manage removal themselves; today no caller uses
/// it, but the affordance is cheap).
pub struct CidEntryGuard<K, V>
where
    K: Eq + Hash,
{
    map: Arc<DashMap<K, V>>,
    keys: [Option<K>; 2],
}

impl<K, V> CidEntryGuard<K, V>
where
    K: Eq + Hash + Clone,
    V: Send + Sync,
{
    /// Build a guard owning the two keys to be removed on drop.
    pub fn new(map: Arc<DashMap<K, V>>, router_key: K, header_dcid_key: K) -> Self {
        Self {
            map,
            keys: [Some(router_key), Some(header_dcid_key)],
        }
    }

    /// Cancel the guard's effect — Drop will do nothing.
    #[allow(dead_code)]
    pub fn disarm(mut self) {
        self.keys = [None, None];
    }
}

impl<K, V> Drop for CidEntryGuard<K, V>
where
    K: Eq + Hash,
{
    fn drop(&mut self) {
        for slot in &mut self.keys {
            if let Some(k) = slot.take() {
                // DashMap::remove is non-async, bounded, and panic-free
                // under hashable+eq keys. Safe to call from a `Drop`
                // that may be running during an unwind.
                self.map.remove(&k);
            }
        }
    }
}
