//! Session persistence trait for sticky routing.

/// Session persistence for sticky routing.
///
/// Maps a session key to a value (typically a node ID or address)
/// so that subsequent requests from the same client are routed
/// to the same backend.
pub trait SessionPersistence<K, V>: Send + Sync {
    /// Get the stored value for the given key, if it exists and hasn't expired.
    fn get(&self, key: &K) -> Option<V>;

    /// Store a key-value mapping.
    fn put(&self, key: K, value: V);

    /// Remove a mapping.
    fn remove(&self, key: &K);

    /// Number of active sessions.
    fn len(&self) -> usize;

    /// Whether the session store is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
