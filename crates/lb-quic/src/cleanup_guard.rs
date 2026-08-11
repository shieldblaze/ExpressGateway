//! CODE-2-08 — RAII drop-guard that removes per-CID DashMap entries unconditionally on actor exit
//! (clean / cancelled / panicked).
//!
//! Before this guard the spawn site removed entries with explicit calls AFTER
//! `run_actor(actor).await`, which never run if `run_actor` panics. The `mpsc::Sender` drop still
//! reaps a CID that sees a second packet, but a CID that never does — idle, NAT-rebind,
//! attacker-injected DCID — is pinned for the router's lifetime, so `2 * max_connections` panics
//! exhaust the router's cap: a denial-of-service via panic exhaustion.
//!
//! Dead code under `panic = "abort"`; kept for dev/test, where unwind is preserved.

use std::hash::Hash;
use std::sync::Arc;

use dashmap::DashMap;

/// RAII removal of up to two DashMap entries on normal scope exit, async-cancel future-drop, or
/// panic unwind. The keys are `Option<K>` so [`Self::disarm`] can `take` them.
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
                // `DashMap::remove` is non-async, bounded and panic-free for hashable+eq keys, so
                // it is safe to call during an unwind.
                self.map.remove(&k);
            }
        }
    }
}
