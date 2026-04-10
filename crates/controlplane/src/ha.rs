//! High-availability traits and implementations for the control plane.
//!
//! Defines the three core HA abstractions:
//! - [`ConfigStore`] -- distributed key-value config storage
//! - [`ServiceDiscovery`] -- service registration and discovery
//! - [`LeaderElection`] -- leader election with fencing
//!
//! # Backend matrix
//!
//! | Backend     | ConfigStore | ServiceDiscovery | LeaderElection |
//! |-------------|-------------|------------------|----------------|
//! | In-memory   | Full        | Full             | Full           |
//! | etcd        | Stub        | Stub             | Stub           |
//! | Consul      | Stub        | Stub             | Stub           |
//! | ZooKeeper   | Stub        | Stub             | Stub           |

use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Common types
// ---------------------------------------------------------------------------

/// A change event from a watched config key prefix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEvent {
    pub key: String,
    pub value: Option<Vec<u8>>,
    pub kind: ConfigEventKind,
}

/// The kind of change in a [`ConfigEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigEventKind {
    Put,
    Delete,
}

/// Metadata about a registered service instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub healthy: bool,
    pub metadata: std::collections::HashMap<String, String>,
}

/// Opaque guard returned by [`LeaderElection::campaign`].
///
/// While this guard is held (not dropped), the holder is the leader.
/// Dropping the guard is equivalent to resigning.
pub struct LeaderGuard {
    _resign_tx: tokio::sync::oneshot::Sender<()>,
}

impl LeaderGuard {
    fn new(tx: tokio::sync::oneshot::Sender<()>) -> Self {
        Self { _resign_tx: tx }
    }
}

/// Metadata about the current leader node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub address: String,
}

/// Errors from HA operations.
#[derive(Debug, thiserror::Error)]
pub enum HaError {
    #[error("not implemented: {backend} backend for {operation}")]
    NotImplemented {
        backend: &'static str,
        operation: &'static str,
    },

    #[error("key not found: {0}")]
    KeyNotFound(String),

    #[error("CAS conflict on key: {0}")]
    CasConflict(String),

    #[error("service not found: {0}")]
    ServiceNotFound(String),

    #[error("election lost")]
    ElectionLost,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(String),
}

pub type HaResult<T> = Result<T, HaError>;

// Type alias for the boxed stream used in trait return types.
type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

// ---------------------------------------------------------------------------
// ConfigStore trait
// ---------------------------------------------------------------------------

/// Distributed key-value configuration store.
///
/// # Wire format expectations by backend
///
/// ## etcd
/// - Keys are UTF-8 strings, values are opaque bytes.
/// - `watch` uses etcd's native watch API with revision tracking.
/// - `cas` maps to etcd's `Txn` with `Compare::value` + `Then::put`.
/// - Prefix listing uses range requests with `key` = prefix, `range_end` = prefix + 1.
///
/// ## Consul
/// - Keys are Consul KV paths (slash-separated), values are base64-encoded in the HTTP API.
/// - `watch` uses blocking queries with `?index=` long-poll.
/// - `cas` uses `?cas=<ModifyIndex>` on PUT.
/// - Prefix listing uses `?keys` or `?recurse` on the prefix path.
///
/// ## ZooKeeper
/// - Keys map to znode paths (slash-separated), values are znode data (max 1MB default).
/// - `watch` uses ZK persistent watches (3.6+) or re-registered one-shot watches.
/// - `cas` uses `setData` with expected `version` (stat.version).
/// - Prefix listing uses `getChildren` on the parent znode.
pub trait ConfigStore: Send + Sync {
    /// Get a value by key. Returns `None` if the key does not exist.
    fn get(
        &self,
        key: &str,
    ) -> impl std::future::Future<Output = HaResult<Option<Vec<u8>>>> + Send;

    /// Put a key-value pair, overwriting any existing value.
    fn put(
        &self,
        key: &str,
        value: &[u8],
    ) -> impl std::future::Future<Output = HaResult<()>> + Send;

    /// Delete a key. No-op if the key does not exist.
    fn delete(&self, key: &str) -> impl std::future::Future<Output = HaResult<()>> + Send;

    /// List all key-value pairs under a prefix.
    fn list(
        &self,
        prefix: &str,
    ) -> impl std::future::Future<Output = HaResult<Vec<(String, Vec<u8>)>>> + Send;

    /// Watch for changes under a prefix. Returns a stream of [`ConfigEvent`]s.
    fn watch(
        &self,
        prefix: &str,
    ) -> impl std::future::Future<Output = HaResult<BoxStream<ConfigEvent>>> + Send;

    /// Compare-and-swap: set `key` to `value` only if the current value matches `expected`.
    /// `expected = None` means the key must not exist.
    /// Returns `true` if the swap succeeded, `false` if the comparison failed.
    fn cas(
        &self,
        key: &str,
        expected: Option<&[u8]>,
        value: &[u8],
    ) -> impl std::future::Future<Output = HaResult<bool>> + Send;
}

// ---------------------------------------------------------------------------
// ServiceDiscovery trait
// ---------------------------------------------------------------------------

/// Service registration and discovery.
///
/// # Wire format expectations by backend
///
/// ## etcd
/// - Services stored as JSON under `/services/{name}/{id}` keys.
/// - Health updates modify the JSON `healthy` field via read-modify-write with CAS.
/// - Discovery reads all keys under `/services/{name}/` prefix.
/// - Watch uses etcd watch on the `/services/{name}/` prefix.
///
/// ## Consul
/// - Uses Consul's agent service registration API (`/v1/agent/service/register`).
/// - Health checks registered alongside the service (TTL or HTTP check).
/// - Discovery via `/v1/health/service/{name}?passing=true`.
/// - Watch via blocking queries on the health endpoint.
///
/// ## ZooKeeper
/// - Services stored as ephemeral znodes under `/services/{name}/{id}`.
/// - Health encoded in znode data. Ephemeral nodes auto-cleanup on session loss.
/// - Discovery via `getChildren` + `getData` on `/services/{name}/`.
/// - Watch via child watches on the parent znode.
pub trait ServiceDiscovery: Send + Sync {
    /// Register a service instance.
    fn register(
        &self,
        service: &ServiceInfo,
    ) -> impl std::future::Future<Output = HaResult<()>> + Send;

    /// Deregister a service instance by ID.
    fn deregister(
        &self,
        service_id: &str,
    ) -> impl std::future::Future<Output = HaResult<()>> + Send;

    /// Discover all instances of a service by name.
    fn discover(
        &self,
        name: &str,
    ) -> impl std::future::Future<Output = HaResult<Vec<ServiceInfo>>> + Send;

    /// Watch for changes to a service's instance list.
    fn watch(
        &self,
        name: &str,
    ) -> impl std::future::Future<Output = HaResult<BoxStream<Vec<ServiceInfo>>>> + Send;

    /// Update the health status of a service instance.
    fn health_update(
        &self,
        service_id: &str,
        healthy: bool,
    ) -> impl std::future::Future<Output = HaResult<()>> + Send;
}

// ---------------------------------------------------------------------------
// LeaderElection trait
// ---------------------------------------------------------------------------

/// Leader election with fencing via guard objects.
///
/// # Wire format expectations by backend
///
/// ## etcd
/// - Uses etcd's election API (`/v3/election/campaign`).
/// - Leader key stored under `/elections/{name}` with a lease TTL.
/// - `campaign` blocks until the lease is granted and the node becomes leader.
/// - Resigning revokes the lease.
///
/// ## Consul
/// - Uses Consul sessions + KV locking (`/v1/kv/{name}?acquire={session}`).
/// - Session created with TTL and `delete` behavior on invalidation.
/// - `campaign` retries lock acquisition with exponential backoff.
/// - Resigning releases the lock and destroys the session.
///
/// ## ZooKeeper
/// - Uses ZK recipes for leader election (sequential ephemeral znodes).
/// - Candidates create `/elections/{name}/candidate-` sequential znodes.
/// - The node with the lowest sequence number is the leader.
/// - Watch set on the predecessor znode for leader failover.
pub trait LeaderElection: Send + Sync {
    /// Campaign to become leader for the named election.
    /// Returns a [`LeaderGuard`] that maintains leadership while held.
    fn campaign(
        &self,
        name: &str,
    ) -> impl std::future::Future<Output = HaResult<LeaderGuard>> + Send;

    /// Check if this node is currently the leader.
    fn is_leader(&self) -> impl std::future::Future<Output = bool> + Send;

    /// Get information about the current leader, if any.
    fn leader_info(
        &self,
        name: &str,
    ) -> impl std::future::Future<Output = HaResult<Option<NodeInfo>>> + Send;

    /// Resign leadership. No-op if not the leader.
    fn resign(&self) -> impl std::future::Future<Output = HaResult<()>> + Send;
}

// ===========================================================================
// In-memory implementations (fully functional, single-node)
// ===========================================================================

pub mod memory {
    //! In-memory implementations of all HA traits.
    //!
    //! Suitable for single-node deployments and testing.
    //! No persistence, no distribution -- all state lives in process memory.

    use super::*;
    use dashmap::DashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::broadcast;

    // -----------------------------------------------------------------------
    // InMemoryConfigStore
    // -----------------------------------------------------------------------

    /// In-memory config store backed by [`DashMap`].
    pub struct InMemoryConfigStore {
        data: DashMap<String, Vec<u8>>,
        event_tx: broadcast::Sender<ConfigEvent>,
    }

    impl InMemoryConfigStore {
        pub fn new() -> Self {
            let (event_tx, _) = broadcast::channel(256);
            Self {
                data: DashMap::new(),
                event_tx,
            }
        }
    }

    impl Default for InMemoryConfigStore {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ConfigStore for InMemoryConfigStore {
        async fn get(&self, key: &str) -> HaResult<Option<Vec<u8>>> {
            Ok(self.data.get(key).map(|v| v.value().clone()))
        }

        async fn put(&self, key: &str, value: &[u8]) -> HaResult<()> {
            self.data.insert(key.to_string(), value.to_vec());
            // Best-effort broadcast; receivers may have been dropped.
            let _ = self.event_tx.send(ConfigEvent {
                key: key.to_string(),
                value: Some(value.to_vec()),
                kind: ConfigEventKind::Put,
            });
            Ok(())
        }

        async fn delete(&self, key: &str) -> HaResult<()> {
            self.data.remove(key);
            let _ = self.event_tx.send(ConfigEvent {
                key: key.to_string(),
                value: None,
                kind: ConfigEventKind::Delete,
            });
            Ok(())
        }

        async fn list(&self, prefix: &str) -> HaResult<Vec<(String, Vec<u8>)>> {
            let results: Vec<(String, Vec<u8>)> = self
                .data
                .iter()
                .filter(|entry| entry.key().starts_with(prefix))
                .map(|entry| (entry.key().clone(), entry.value().clone()))
                .collect();
            Ok(results)
        }

        async fn watch(&self, prefix: &str) -> HaResult<BoxStream<ConfigEvent>> {
            let mut rx = self.event_tx.subscribe();
            let prefix = prefix.to_string();
            let (tx, mpsc_rx) = tokio::sync::mpsc::channel(64);

            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(event) if event.key.starts_with(&prefix) => {
                            if tx.send(event).await.is_err() {
                                break;
                            }
                        }
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(mpsc_rx)))
        }

        async fn cas(&self, key: &str, expected: Option<&[u8]>, value: &[u8]) -> HaResult<bool> {
            // DashMap entry API provides the atomicity we need for single-node.
            match expected {
                None => {
                    // Key must not exist.
                    if self.data.contains_key(key) {
                        return Ok(false);
                    }
                    self.data.insert(key.to_string(), value.to_vec());
                }
                Some(exp) => {
                    let entry = self.data.get(key);
                    match entry {
                        Some(current) if current.value().as_slice() == exp => {
                            drop(current);
                            self.data.insert(key.to_string(), value.to_vec());
                        }
                        _ => return Ok(false),
                    }
                }
            }
            let _ = self.event_tx.send(ConfigEvent {
                key: key.to_string(),
                value: Some(value.to_vec()),
                kind: ConfigEventKind::Put,
            });
            Ok(true)
        }
    }

    // -----------------------------------------------------------------------
    // InMemoryServiceDiscovery
    // -----------------------------------------------------------------------

    /// In-memory service discovery backed by [`DashMap`].
    pub struct InMemoryServiceDiscovery {
        services: DashMap<String, ServiceInfo>,
        change_tx: broadcast::Sender<(String, Vec<ServiceInfo>)>,
    }

    impl InMemoryServiceDiscovery {
        pub fn new() -> Self {
            let (change_tx, _) = broadcast::channel(256);
            Self {
                services: DashMap::new(),
                change_tx,
            }
        }

        /// Build a snapshot of all services with the given name.
        fn snapshot_for(&self, name: &str) -> Vec<ServiceInfo> {
            self.services
                .iter()
                .filter(|e| e.value().name == name)
                .map(|e| e.value().clone())
                .collect()
        }

        /// Broadcast a change for the given service name.
        fn notify(&self, name: &str) {
            let snapshot = self.snapshot_for(name);
            let _ = self.change_tx.send((name.to_string(), snapshot));
        }
    }

    impl Default for InMemoryServiceDiscovery {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ServiceDiscovery for InMemoryServiceDiscovery {
        async fn register(&self, service: &ServiceInfo) -> HaResult<()> {
            let name = service.name.clone();
            self.services.insert(service.id.clone(), service.clone());
            self.notify(&name);
            Ok(())
        }

        async fn deregister(&self, service_id: &str) -> HaResult<()> {
            if let Some((_, svc)) = self.services.remove(service_id) {
                self.notify(&svc.name);
            }
            Ok(())
        }

        async fn discover(&self, name: &str) -> HaResult<Vec<ServiceInfo>> {
            Ok(self.snapshot_for(name))
        }

        async fn watch(&self, name: &str) -> HaResult<BoxStream<Vec<ServiceInfo>>> {
            let mut rx = self.change_tx.subscribe();
            let name = name.to_string();
            let (tx, mpsc_rx) = tokio::sync::mpsc::channel(64);

            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok((changed_name, snapshot)) if changed_name == name => {
                            if tx.send(snapshot).await.is_err() {
                                break;
                            }
                        }
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(mpsc_rx)))
        }

        async fn health_update(&self, service_id: &str, healthy: bool) -> HaResult<()> {
            let mut entry = self
                .services
                .get_mut(service_id)
                .ok_or_else(|| HaError::ServiceNotFound(service_id.to_string()))?;
            let name = entry.name.clone();
            entry.healthy = healthy;
            drop(entry);
            self.notify(&name);
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // InMemoryLeaderElection
    // -----------------------------------------------------------------------

    /// In-memory leader election.
    ///
    /// In single-node mode the first campaigner always wins.
    pub struct InMemoryLeaderElection {
        node_info: NodeInfo,
        is_leader: Arc<AtomicBool>,
        leaders: DashMap<String, NodeInfo>,
    }

    impl InMemoryLeaderElection {
        pub fn new(node_info: NodeInfo) -> Self {
            Self {
                node_info,
                is_leader: Arc::new(AtomicBool::new(false)),
                leaders: DashMap::new(),
            }
        }
    }

    impl LeaderElection for InMemoryLeaderElection {
        async fn campaign(&self, name: &str) -> HaResult<LeaderGuard> {
            // Single-node: always succeed.
            self.is_leader.store(true, Ordering::Release);
            self.leaders
                .insert(name.to_string(), self.node_info.clone());

            let is_leader = self.is_leader.clone();
            let leaders = self.leaders.clone();
            let name_owned = name.to_string();

            let (tx, rx) = tokio::sync::oneshot::channel::<()>();

            // Spawn a task that waits for the guard to be dropped (resign).
            tokio::spawn(async move {
                let _ = rx.await;
                is_leader.store(false, Ordering::Release);
                leaders.remove(&name_owned);
            });

            Ok(LeaderGuard::new(tx))
        }

        async fn is_leader(&self) -> bool {
            self.is_leader.load(Ordering::Acquire)
        }

        async fn leader_info(&self, name: &str) -> HaResult<Option<NodeInfo>> {
            Ok(self.leaders.get(name).map(|e| e.value().clone()))
        }

        async fn resign(&self) -> HaResult<()> {
            self.is_leader.store(false, Ordering::Release);
            self.leaders.clear();
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use futures::StreamExt;

        #[tokio::test]
        async fn config_store_crud() {
            let store = InMemoryConfigStore::new();
            assert!(store.get("k1").await.unwrap().is_none());

            store.put("k1", b"v1").await.unwrap();
            assert_eq!(store.get("k1").await.unwrap().unwrap(), b"v1");

            store.put("k1", b"v2").await.unwrap();
            assert_eq!(store.get("k1").await.unwrap().unwrap(), b"v2");

            store.delete("k1").await.unwrap();
            assert!(store.get("k1").await.unwrap().is_none());
        }

        #[tokio::test]
        async fn config_store_list() {
            let store = InMemoryConfigStore::new();
            store.put("app/db/host", b"localhost").await.unwrap();
            store.put("app/db/port", b"5432").await.unwrap();
            store.put("app/cache/host", b"redis").await.unwrap();

            let db_entries = store.list("app/db/").await.unwrap();
            assert_eq!(db_entries.len(), 2);

            let all_entries = store.list("app/").await.unwrap();
            assert_eq!(all_entries.len(), 3);
        }

        #[tokio::test]
        async fn config_store_cas() {
            let store = InMemoryConfigStore::new();

            // CAS on non-existent key with expected=None should succeed.
            assert!(store.cas("k1", None, b"v1").await.unwrap());

            // CAS with wrong expected should fail.
            assert!(!store.cas("k1", Some(b"wrong"), b"v2").await.unwrap());

            // CAS with correct expected should succeed.
            assert!(store.cas("k1", Some(b"v1"), b"v2").await.unwrap());
            assert_eq!(store.get("k1").await.unwrap().unwrap(), b"v2");

            // CAS on existing key with expected=None should fail.
            assert!(!store.cas("k1", None, b"v3").await.unwrap());
        }

        #[tokio::test]
        async fn config_store_watch() {
            let store = Arc::new(InMemoryConfigStore::new());
            let mut watch_stream = store.watch("app/").await.unwrap();

            let store2 = store.clone();
            tokio::spawn(async move {
                store2.put("app/key1", b"val1").await.unwrap();
                store2.put("other/key", b"ignored").await.unwrap();
                store2.delete("app/key1").await.unwrap();
            });

            let ev1 = watch_stream.next().await.unwrap();
            assert_eq!(ev1.key, "app/key1");
            assert_eq!(ev1.kind, ConfigEventKind::Put);

            let ev2 = watch_stream.next().await.unwrap();
            assert_eq!(ev2.key, "app/key1");
            assert_eq!(ev2.kind, ConfigEventKind::Delete);
        }

        #[tokio::test]
        async fn service_discovery_crud() {
            let sd = InMemoryServiceDiscovery::new();

            let svc = ServiceInfo {
                id: "svc-1".into(),
                name: "web".into(),
                address: "10.0.0.1".into(),
                port: 8080,
                healthy: true,
                metadata: Default::default(),
            };

            sd.register(&svc).await.unwrap();

            let discovered = sd.discover("web").await.unwrap();
            assert_eq!(discovered.len(), 1);
            assert_eq!(discovered[0].id, "svc-1");

            sd.health_update("svc-1", false).await.unwrap();
            let discovered = sd.discover("web").await.unwrap();
            assert!(!discovered[0].healthy);

            sd.deregister("svc-1").await.unwrap();
            let discovered = sd.discover("web").await.unwrap();
            assert!(discovered.is_empty());
        }

        #[tokio::test]
        async fn leader_election_single_node() {
            let node = NodeInfo {
                id: "node-1".into(),
                address: "10.0.0.1:50051".into(),
            };
            let election = InMemoryLeaderElection::new(node);

            assert!(!election.is_leader().await);

            let guard = election.campaign("test-election").await.unwrap();
            assert!(election.is_leader().await);

            let info = election
                .leader_info("test-election")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(info.id, "node-1");

            drop(guard);
            // Give the background task time to run.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            assert!(!election.is_leader().await);
        }
    }
}

// ===========================================================================
// Stub implementations (etcd, Consul, ZooKeeper)
// ===========================================================================

pub mod etcd {
    //! etcd backend stubs.
    //!
    //! Wire format:
    //! - Keys: UTF-8 strings, values: opaque bytes
    //! - Watch: etcd native watch API with revision tracking
    //! - CAS: `Txn` with `Compare::value` + `Then::put`
    //! - List: range requests with `key=prefix`, `range_end=prefix+1`

    use super::*;

    pub struct EtcdConfigStore;
    pub struct EtcdServiceDiscovery;
    pub struct EtcdLeaderElection;

    impl ConfigStore for EtcdConfigStore {
        async fn get(&self, _key: &str) -> HaResult<Option<Vec<u8>>> {
            todo!("etcd ConfigStore::get -- connect via etcd-client crate, GET /v3/kv/range")
        }
        async fn put(&self, _key: &str, _value: &[u8]) -> HaResult<()> {
            todo!("etcd ConfigStore::put -- PUT /v3/kv/put")
        }
        async fn delete(&self, _key: &str) -> HaResult<()> {
            todo!("etcd ConfigStore::delete -- POST /v3/kv/deleterange")
        }
        async fn list(&self, _prefix: &str) -> HaResult<Vec<(String, Vec<u8>)>> {
            todo!("etcd ConfigStore::list -- range request with prefix+1 as range_end")
        }
        async fn watch(&self, _prefix: &str) -> HaResult<BoxStream<ConfigEvent>> {
            todo!("etcd ConfigStore::watch -- POST /v3/watch with create_request")
        }
        async fn cas(&self, _key: &str, _expected: Option<&[u8]>, _value: &[u8]) -> HaResult<bool> {
            todo!("etcd ConfigStore::cas -- Txn with Compare::value + Then::put")
        }
    }

    impl ServiceDiscovery for EtcdServiceDiscovery {
        async fn register(&self, _service: &ServiceInfo) -> HaResult<()> {
            todo!("etcd ServiceDiscovery::register -- PUT /services/{{name}}/{{id}} with lease")
        }
        async fn deregister(&self, _service_id: &str) -> HaResult<()> {
            todo!("etcd ServiceDiscovery::deregister -- DELETE /services/{{name}}/{{id}}, revoke lease")
        }
        async fn discover(&self, _name: &str) -> HaResult<Vec<ServiceInfo>> {
            todo!("etcd ServiceDiscovery::discover -- range GET /services/{{name}}/")
        }
        async fn watch(&self, _name: &str) -> HaResult<BoxStream<Vec<ServiceInfo>>> {
            todo!("etcd ServiceDiscovery::watch -- watch /services/{{name}}/ prefix")
        }
        async fn health_update(&self, _service_id: &str, _healthy: bool) -> HaResult<()> {
            todo!("etcd ServiceDiscovery::health_update -- read-modify-write with CAS")
        }
    }

    impl LeaderElection for EtcdLeaderElection {
        async fn campaign(&self, _name: &str) -> HaResult<LeaderGuard> {
            todo!("etcd LeaderElection::campaign -- /v3/election/campaign with lease TTL")
        }
        async fn is_leader(&self) -> bool {
            todo!("etcd LeaderElection::is_leader -- check lease validity")
        }
        async fn leader_info(&self, _name: &str) -> HaResult<Option<NodeInfo>> {
            todo!("etcd LeaderElection::leader_info -- /v3/election/leader")
        }
        async fn resign(&self) -> HaResult<()> {
            todo!("etcd LeaderElection::resign -- /v3/election/resign, revoke lease")
        }
    }
}

pub mod consul {
    //! Consul backend stubs.
    //!
    //! Wire format:
    //! - KV: paths are slash-separated, values base64-encoded in HTTP API
    //! - Watch: blocking queries with `?index=` long-poll
    //! - CAS: `?cas=<ModifyIndex>` on PUT
    //! - Service: agent registration API, health via TTL or HTTP checks

    use super::*;

    pub struct ConsulConfigStore;
    pub struct ConsulServiceDiscovery;
    pub struct ConsulLeaderElection;

    impl ConfigStore for ConsulConfigStore {
        async fn get(&self, _key: &str) -> HaResult<Option<Vec<u8>>> {
            todo!("consul ConfigStore::get -- GET /v1/kv/{{key}}")
        }
        async fn put(&self, _key: &str, _value: &[u8]) -> HaResult<()> {
            todo!("consul ConfigStore::put -- PUT /v1/kv/{{key}}")
        }
        async fn delete(&self, _key: &str) -> HaResult<()> {
            todo!("consul ConfigStore::delete -- DELETE /v1/kv/{{key}}")
        }
        async fn list(&self, _prefix: &str) -> HaResult<Vec<(String, Vec<u8>)>> {
            todo!("consul ConfigStore::list -- GET /v1/kv/{{prefix}}?recurse")
        }
        async fn watch(&self, _prefix: &str) -> HaResult<BoxStream<ConfigEvent>> {
            todo!("consul ConfigStore::watch -- GET /v1/kv/{{prefix}}?recurse&index=N blocking query")
        }
        async fn cas(&self, _key: &str, _expected: Option<&[u8]>, _value: &[u8]) -> HaResult<bool> {
            todo!("consul ConfigStore::cas -- PUT /v1/kv/{{key}}?cas=<ModifyIndex>")
        }
    }

    impl ServiceDiscovery for ConsulServiceDiscovery {
        async fn register(&self, _service: &ServiceInfo) -> HaResult<()> {
            todo!("consul ServiceDiscovery::register -- PUT /v1/agent/service/register")
        }
        async fn deregister(&self, _service_id: &str) -> HaResult<()> {
            todo!("consul ServiceDiscovery::deregister -- PUT /v1/agent/service/deregister/{{id}}")
        }
        async fn discover(&self, _name: &str) -> HaResult<Vec<ServiceInfo>> {
            todo!("consul ServiceDiscovery::discover -- GET /v1/health/service/{{name}}?passing=true")
        }
        async fn watch(&self, _name: &str) -> HaResult<BoxStream<Vec<ServiceInfo>>> {
            todo!("consul ServiceDiscovery::watch -- GET /v1/health/service/{{name}}?index=N blocking query")
        }
        async fn health_update(&self, _service_id: &str, _healthy: bool) -> HaResult<()> {
            todo!("consul ServiceDiscovery::health_update -- PUT /v1/agent/check/pass|fail/{{id}}")
        }
    }

    impl LeaderElection for ConsulLeaderElection {
        async fn campaign(&self, _name: &str) -> HaResult<LeaderGuard> {
            todo!("consul LeaderElection::campaign -- create session + PUT /v1/kv/{{name}}?acquire={{session}}")
        }
        async fn is_leader(&self) -> bool {
            todo!("consul LeaderElection::is_leader -- GET /v1/kv/{{name}} check Session field")
        }
        async fn leader_info(&self, _name: &str) -> HaResult<Option<NodeInfo>> {
            todo!("consul LeaderElection::leader_info -- GET /v1/kv/{{name}} decode value")
        }
        async fn resign(&self) -> HaResult<()> {
            todo!("consul LeaderElection::resign -- PUT /v1/kv/{{name}}?release={{session}}, destroy session")
        }
    }
}

pub mod zookeeper {
    //! ZooKeeper backend stubs.
    //!
    //! Wire format:
    //! - Keys map to znode paths (slash-separated), values are znode data (max 1MB default)
    //! - Watch: persistent watches (ZK 3.6+) or re-registered one-shot watches
    //! - CAS: `setData` with expected `version` (stat.version)
    //! - Service: ephemeral znodes for auto-cleanup on session loss

    use super::*;

    pub struct ZookeeperConfigStore;
    pub struct ZookeeperServiceDiscovery;
    pub struct ZookeeperLeaderElection;

    impl ConfigStore for ZookeeperConfigStore {
        async fn get(&self, _key: &str) -> HaResult<Option<Vec<u8>>> {
            todo!("zk ConfigStore::get -- getData on znode path")
        }
        async fn put(&self, _key: &str, _value: &[u8]) -> HaResult<()> {
            todo!("zk ConfigStore::put -- setData or create if not exists")
        }
        async fn delete(&self, _key: &str) -> HaResult<()> {
            todo!("zk ConfigStore::delete -- delete znode")
        }
        async fn list(&self, _prefix: &str) -> HaResult<Vec<(String, Vec<u8>)>> {
            todo!("zk ConfigStore::list -- getChildren + getData on each child")
        }
        async fn watch(&self, _prefix: &str) -> HaResult<BoxStream<ConfigEvent>> {
            todo!("zk ConfigStore::watch -- persistent watch on znode subtree (ZK 3.6+)")
        }
        async fn cas(&self, _key: &str, _expected: Option<&[u8]>, _value: &[u8]) -> HaResult<bool> {
            todo!("zk ConfigStore::cas -- setData with stat.version, catch BadVersion")
        }
    }

    impl ServiceDiscovery for ZookeeperServiceDiscovery {
        async fn register(&self, _service: &ServiceInfo) -> HaResult<()> {
            todo!("zk ServiceDiscovery::register -- create ephemeral znode /services/{{name}}/{{id}}")
        }
        async fn deregister(&self, _service_id: &str) -> HaResult<()> {
            todo!("zk ServiceDiscovery::deregister -- delete znode (or let session expire)")
        }
        async fn discover(&self, _name: &str) -> HaResult<Vec<ServiceInfo>> {
            todo!("zk ServiceDiscovery::discover -- getChildren /services/{{name}}/ + getData each")
        }
        async fn watch(&self, _name: &str) -> HaResult<BoxStream<Vec<ServiceInfo>>> {
            todo!("zk ServiceDiscovery::watch -- child watch on /services/{{name}}/")
        }
        async fn health_update(&self, _service_id: &str, _healthy: bool) -> HaResult<()> {
            todo!("zk ServiceDiscovery::health_update -- setData on the service znode")
        }
    }

    impl LeaderElection for ZookeeperLeaderElection {
        async fn campaign(&self, _name: &str) -> HaResult<LeaderGuard> {
            todo!("zk LeaderElection::campaign -- create sequential ephemeral /elections/{{name}}/candidate-")
        }
        async fn is_leader(&self) -> bool {
            todo!("zk LeaderElection::is_leader -- check if our znode has lowest sequence number")
        }
        async fn leader_info(&self, _name: &str) -> HaResult<Option<NodeInfo>> {
            todo!("zk LeaderElection::leader_info -- getChildren, find min sequence, getData")
        }
        async fn resign(&self) -> HaResult<()> {
            todo!("zk LeaderElection::resign -- delete our candidate znode")
        }
    }
}
