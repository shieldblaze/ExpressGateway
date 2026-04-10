/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.controlplane.kvstore.zookeeper;

import com.shieldblaze.expressgateway.common.utils.LogSanitizer;
import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatchEvent;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatcher;
import lombok.extern.log4j.Log4j2;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.recipes.cache.ChildData;
import org.apache.curator.framework.recipes.cache.CuratorCache;
import org.apache.curator.framework.recipes.cache.CuratorCacheListener;
import org.apache.curator.framework.recipes.leader.LeaderLatch;
import org.apache.curator.framework.recipes.leader.LeaderLatchListener;
import org.apache.curator.framework.recipes.locks.InterProcessMutex;
import org.apache.zookeeper.CreateMode;
import org.apache.zookeeper.KeeperException;
import org.apache.zookeeper.data.Stat;

import java.io.Closeable;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;

/**
 * ZooKeeper-backed {@link KVStore} implementation using Apache Curator.
 *
 * <p>Key mapping: KV keys map directly to ZNode paths. The ZNode's {@code stat.version}
 * field is used as the KV version (monotonically incrementing on each update).
 * {@code stat.version} starts at 0 after creation and increments by 1 on each
 * {@code setData} call, so the version returned from {@link #put} and {@link #cas}
 * is the ZNode's version after the write.</p>
 *
 * <p>Thread safety: all operations delegate to the thread-safe {@link CuratorFramework}.
 * Watch callbacks and leader election listeners may fire from Curator's internal threads.</p>
 */
@Log4j2
public final class ZooKeeperKVStore implements KVStore {

    private static final int LOCK_ACQUIRE_TIMEOUT_SECONDS = 30;

    @Override
    public Optional<KVEntry> get(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        try {
            CuratorFramework curator = curator();
            Stat stat = curator.checkExists().forPath(key);
            if (stat == null) {
                return Optional.empty();
            }
            Stat readStat = new Stat();
            byte[] data = curator.getData().storingStatIn(readStat).forPath(key);
            return Optional.of(toEntry(key, data, readStat));
        } catch (KeeperException.NoNodeException e) {
            // Race: node deleted between checkExists and getData
            return Optional.empty();
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public List<KVEntry> list(String prefix) throws KVStoreException {
        Objects.requireNonNull(prefix, "prefix");
        // ZooKeeper/Curator rejects paths ending with '/'; normalize by stripping trailing slash.
        if (prefix.length() > 1 && prefix.endsWith("/")) {
            prefix = prefix.substring(0, prefix.length() - 1);
        }
        try {
            CuratorFramework curator = curator();
            Stat stat = curator.checkExists().forPath(prefix);
            if (stat == null) {
                return Collections.emptyList();
            }
            List<String> children = curator.getChildren().forPath(prefix);
            if (children.isEmpty()) {
                return Collections.emptyList();
            }
            List<KVEntry> entries = new ArrayList<>(children.size());
            for (String child : children) {
                String childPath = prefix.endsWith("/") ? prefix + child : prefix + "/" + child;
                try {
                    Stat childStat = new Stat();
                    byte[] data = curator.getData().storingStatIn(childStat).forPath(childPath);
                    entries.add(toEntry(childPath, data, childStat));
                } catch (KeeperException.NoNodeException e) {
                    // Child was deleted between getChildren and getData -- skip it
                    log.debug("Child node {} disappeared during list, skipping", LogSanitizer.sanitize(childPath));
                }
            }
            return entries;
        } catch (Exception e) {
            throw mapException(e, prefix);
        }
    }

    @Override
    public List<KVEntry> listRecursive(String prefix) throws KVStoreException {
        Objects.requireNonNull(prefix, "prefix");
        if (prefix.length() > 1 && prefix.endsWith("/")) {
            prefix = prefix.substring(0, prefix.length() - 1);
        }
        try {
            CuratorFramework curator = curator();
            Stat stat = curator.checkExists().forPath(prefix);
            if (stat == null) {
                return Collections.emptyList();
            }
            List<KVEntry> entries = new ArrayList<>();
            collectChildrenRecursive(curator, prefix, entries);
            return entries;
        } catch (Exception e) {
            throw mapException(e, prefix);
        }
    }

    /**
     * Recursively collects all descendant nodes under the given path.
     */
    private static void collectChildrenRecursive(CuratorFramework curator, String path,
                                                  List<KVEntry> entries) throws Exception {
        List<String> children;
        try {
            children = curator.getChildren().forPath(path);
        } catch (KeeperException.NoNodeException e) {
            // Node was deleted concurrently -- skip
            return;
        }
        for (String child : children) {
            String childPath = path.endsWith("/") ? path + child : path + "/" + child;
            try {
                Stat childStat = new Stat();
                byte[] data = curator.getData().storingStatIn(childStat).forPath(childPath);
                entries.add(toEntry(childPath, data, childStat));
                // Recurse into this child
                collectChildrenRecursive(curator, childPath, entries);
            } catch (KeeperException.NoNodeException e) {
                // Child was deleted between getChildren and getData -- skip
                log.debug("Child node {} disappeared during listRecursive, skipping", LogSanitizer.sanitize(childPath));
            }
        }
    }

    @Override
    public long put(String key, byte[] value) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        try {
            CuratorFramework curator = curator();
            // Try setData first (the common case for existing nodes).
            // If the node does not exist, fall through to create.
            Stat existing = curator.checkExists().forPath(key);
            if (existing != null) {
                Stat stat = curator.setData().forPath(key, value);
                return stat.getVersion() + 1L;
            }
            // Node does not exist -- create with parents.
            try {
                curator.create()
                        .creatingParentsIfNeeded()
                        .withMode(CreateMode.PERSISTENT)
                        .forPath(key, value);
                // After create, ZNode version is 0 -> offset to 1
                return 1;
            } catch (KeeperException.NodeExistsException e) {
                // Race: another writer created it between checkExists and create.
                // Retry as setData -- the node definitely exists now.
                Stat stat = curator.setData().forPath(key, value);
                return stat.getVersion() + 1L;
            }
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public long cas(String key, byte[] value, long expectedVersion) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        if (expectedVersion < 0) {
            throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                    "expectedVersion must be >= 0, got: " + expectedVersion);
        }
        if (expectedVersion > Integer.MAX_VALUE) {
            throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                    "expectedVersion exceeds ZooKeeper int range: " + expectedVersion);
        }
        try {
            CuratorFramework curator = curator();
            if (expectedVersion == 0) {
                // Create-if-absent: the key must not exist.
                // If it already exists, create() throws NodeExistsException which we
                // translate to VERSION_CONFLICT.
                curator.create()
                        .creatingParentsIfNeeded()
                        .withMode(CreateMode.PERSISTENT)
                        .forPath(key, value);
                // After create, ZNode version is 0 -> offset to 1
                return 1;
            } else {
                // CAS update: setData with the ZK version check.
                // Subtract the +1 offset to get the actual ZK version for comparison.
                int zkVersion = (int) (expectedVersion - 1);
                Stat stat = curator.setData()
                        .withVersion(zkVersion)
                        .forPath(key, value);
                return stat.getVersion() + 1L;
            }
        } catch (KeeperException.NodeExistsException e) {
            throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                    "Key already exists (CAS with expectedVersion=0): " + key, e);
        } catch (KeeperException.BadVersionException e) {
            throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                    "Version mismatch for key: " + key + ", expectedVersion=" + expectedVersion, e);
        } catch (KeeperException.NoNodeException e) {
            throw new KVStoreException(KVStoreException.Code.KEY_NOT_FOUND,
                    "Key not found for CAS update: " + key, e);
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public boolean delete(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        try {
            CuratorFramework curator = curator();
            if (curator.checkExists().forPath(key) == null) {
                return false;
            }
            curator.delete().forPath(key);
            return true;
        } catch (KeeperException.NoNodeException e) {
            // Race: deleted between checkExists and delete
            return false;
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public void deleteTree(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        if (key.length() > 1 && key.endsWith("/")) {
            key = key.substring(0, key.length() - 1);
        }
        try {
            CuratorFramework curator = curator();
            if (curator.checkExists().forPath(key) == null) {
                return;
            }
            curator.delete().deletingChildrenIfNeeded().forPath(key);
        } catch (KeeperException.NoNodeException e) {
            // Already gone -- idempotent
            log.debug("Node {} already deleted during deleteTree", key);
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public Closeable watch(String keyOrPrefix, KVWatcher watcher) throws KVStoreException {
        Objects.requireNonNull(keyOrPrefix, "keyOrPrefix");
        Objects.requireNonNull(watcher, "watcher");
        // ZooKeeper/Curator rejects paths ending with '/'; normalize by stripping trailing slash.
        final String watchPath = (keyOrPrefix.length() > 1 && keyOrPrefix.endsWith("/"))
                ? keyOrPrefix.substring(0, keyOrPrefix.length() - 1)
                : keyOrPrefix;
        try {
            CuratorFramework curator = curator();
            CuratorCache cache = CuratorCache.build(curator, watchPath);

            CuratorCacheListener listener = CuratorCacheListener.builder()
                    .forCreates(childData -> {
                        try {
                            KVEntry entry = toEntry(childData.getPath(), childData.getData(), childData.getStat());
                            watcher.onEvent(new KVWatchEvent(KVWatchEvent.Type.PUT, entry, null));
                        } catch (Exception e) {
                            log.error("Error dispatching watch CREATE event for path {}", childData.getPath(), e);
                        }
                    })
                    .forChanges((oldData, newData) -> {
                        try {
                            KVEntry previous = toEntry(oldData.getPath(), oldData.getData(), oldData.getStat());
                            KVEntry current = toEntry(newData.getPath(), newData.getData(), newData.getStat());
                            watcher.onEvent(new KVWatchEvent(KVWatchEvent.Type.PUT, current, previous));
                        } catch (Exception e) {
                            log.error("Error dispatching watch CHANGE event for path {}", newData.getPath(), e);
                        }
                    })
                    .forDeletes(oldData -> {
                        try {
                            KVEntry previous = toEntry(oldData.getPath(), oldData.getData(), oldData.getStat());
                            watcher.onEvent(new KVWatchEvent(KVWatchEvent.Type.DELETE, null, previous));
                        } catch (Exception e) {
                            log.error("Error dispatching watch DELETE event for path {}", oldData.getPath(), e);
                        }
                    })
                    .build();

            cache.listenable().addListener(listener);
            cache.start();

            log.debug("Installed watch on {}", watchPath);

            return () -> {
                log.debug("Closing watch on {}", watchPath);
                cache.close();
            };
        } catch (Exception e) {
            throw mapException(e, watchPath);
        }
    }

    @Override
    public Closeable acquireLock(String lockPath) throws KVStoreException {
        Objects.requireNonNull(lockPath, "lockPath");
        try {
            CuratorFramework curator = curator();
            InterProcessMutex mutex = new InterProcessMutex(curator, lockPath);
            if (!mutex.acquire(LOCK_ACQUIRE_TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
                throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Failed to acquire lock within " + LOCK_ACQUIRE_TIMEOUT_SECONDS + "s: " + lockPath);
            }
            log.debug("Acquired lock on {}", lockPath);
            return () -> {
                try {
                    mutex.release();
                    log.debug("Released lock on {}", lockPath);
                } catch (Exception e) {
                    log.error("Error releasing lock on {}", lockPath, e);
                }
            };
        } catch (KVStoreException e) {
            throw e;
        } catch (Exception e) {
            throw mapException(e, lockPath);
        }
    }

    @Override
    public LeaderElection leaderElection(String electionPath, String participantId) throws KVStoreException {
        Objects.requireNonNull(electionPath, "electionPath");
        Objects.requireNonNull(participantId, "participantId");
        try {
            CuratorFramework curator = curator();
            LeaderLatch latch = new LeaderLatch(curator, electionPath, participantId);

            ZooKeeperLeaderElection election = new ZooKeeperLeaderElection(latch);
            latch.addListener(new LeaderLatchListener() {
                @Override
                public void isLeader() {
                    log.info("Participant {} became leader on path {}", participantId, electionPath);
                    election.notifyListeners(true);
                }

                @Override
                public void notLeader() {
                    log.info("Participant {} lost leadership on path {}", participantId, electionPath);
                    election.notifyListeners(false);
                }
            });

            latch.start();
            log.debug("Started leader election on {} with participant {}", electionPath, participantId);
            return election;
        } catch (Exception e) {
            throw mapException(e, electionPath);
        }
    }

    @Override
    public void close() throws IOException {
        // No instance-level resources to clean up; CuratorFramework lifecycle is
        // managed by the Curator singleton. Individual watches, locks, and elections
        // are closed via their returned Closeable handles.
        log.debug("ZooKeeperKVStore closed");
    }

    // ---- Internal helpers ----

    private static CuratorFramework curator() throws KVStoreException {
        try {
            return Curator.getInstance();
        } catch (Exception e) {
            throw new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                    "Failed to obtain CuratorFramework instance", e);
        }
    }

    /**
     * ZooKeeper versions start at 0, but the KVStore contract reserves version 0 for
     * create-if-absent semantics in CAS operations.  We offset by +1 so returned
     * versions are always >= 1 and version 0 is unambiguously "does not exist".
     */
    private static KVEntry toEntry(String path, byte[] data, Stat stat) {
        return new KVEntry(path, data, stat.getVersion() + 1L, stat.getCzxid());
    }

    /**
     * Maps ZooKeeper / Curator exceptions to {@link KVStoreException} with the appropriate code.
     */
    private static KVStoreException mapException(Exception e, String key) {
        return switch (e) {
            case KeeperException.NoNodeException _ ->
                    new KVStoreException(KVStoreException.Code.KEY_NOT_FOUND,
                            "Key not found: " + key, e);
            case KeeperException.BadVersionException _ ->
                    new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                            "Version conflict for key: " + key, e);
            case KeeperException.ConnectionLossException _, KeeperException.SessionExpiredException _ ->
                    new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                            "Connection lost while accessing key: " + key, e);
            case KeeperException.NoAuthException _, KeeperException.AuthFailedException _ ->
                    new KVStoreException(KVStoreException.Code.UNAUTHORIZED,
                            "Unauthorized access to key: " + key, e);
            case KeeperException.OperationTimeoutException _ ->
                    new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                            "Operation timed out for key: " + key, e);
            case InterruptedException _ -> {
                Thread.currentThread().interrupt();
                yield new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Operation interrupted for key: " + key, e);
            }
            default -> new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                    "Internal error for key: " + key, e);
        };
    }

    // ---- LeaderElection wrapper over LeaderLatch ----

    private static final class ZooKeeperLeaderElection implements LeaderElection {

        private final LeaderLatch latch;
        private final CopyOnWriteArrayList<LeaderChangeListener> listeners;
        private volatile boolean closed;

        ZooKeeperLeaderElection(LeaderLatch latch) {
            this.latch = latch;
            this.listeners = new CopyOnWriteArrayList<>();
        }

        @Override
        public boolean isLeader() {
            if (closed) {
                return false;
            }
            try {
                return latch.hasLeadership();
            } catch (IllegalStateException _) {
                // LeaderLatch throws if not started or already closed
                return false;
            }
        }

        @Override
        public String currentLeaderId() throws KVStoreException {
            if (closed) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Leader election is closed");
            }
            try {
                return latch.getLeader().getId();
            } catch (KeeperException.NoNodeException e) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "No leader currently elected", e);
            } catch (Exception e) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Failed to determine current leader", e);
            }
        }

        @Override
        public void addListener(LeaderChangeListener listener) {
            Objects.requireNonNull(listener, "listener");
            listeners.add(listener);
        }

        void notifyListeners(boolean isLeader) {
            for (LeaderChangeListener listener : listeners) {
                try {
                    listener.onLeaderChange(isLeader);
                } catch (Exception e) {
                    log.error("Error notifying leader change listener", e);
                }
            }
        }

        @Override
        public void close() throws IOException {
            if (closed) {
                return;
            }
            closed = true;
            latch.close(LeaderLatch.CloseMode.NOTIFY_LEADER);
        }
    }
}
