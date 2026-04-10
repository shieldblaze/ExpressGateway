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
package com.shieldblaze.expressgateway.coordination.zookeeper;

import com.shieldblaze.expressgateway.coordination.ConnectionListener;
import com.shieldblaze.expressgateway.coordination.CoordinationEntry;
import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.CoordinationProvider;
import com.shieldblaze.expressgateway.coordination.DistributedLock;
import com.shieldblaze.expressgateway.coordination.LeaderElection;
import com.shieldblaze.expressgateway.coordination.WatchEvent;
import lombok.extern.log4j.Log4j2;
import org.apache.curator.RetryPolicy;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.framework.recipes.cache.CuratorCache;
import org.apache.curator.framework.recipes.cache.CuratorCacheListener;
import org.apache.curator.framework.recipes.leader.LeaderLatch;
import org.apache.curator.framework.recipes.locks.InterProcessMutex;
import org.apache.curator.framework.state.ConnectionState;
import org.apache.curator.framework.state.ConnectionStateListener;
import org.apache.curator.retry.RetryNTimes;
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
import java.util.function.Consumer;

/**
 * ZooKeeper-backed {@link CoordinationProvider} implementation using Apache Curator 5.9.
 *
 * <h2>Key/Version mapping</h2>
 * <p>Keys map directly to ZNode paths. The ZNode's {@code stat.version} field is used as
 * the key version. ZK versions start at 0 after creation and increment by 1 on each
 * {@code setData} call. We offset by +1 so returned versions are always >= 1, reserving
 * version 0 for "does not exist" semantics in CAS operations.</p>
 *
 * <h2>Thread safety</h2>
 * <p>All operations delegate to the thread-safe {@link CuratorFramework}. Watch callbacks
 * and leader election listeners may fire from Curator's internal threads.</p>
 *
 * <h2>Ownership</h2>
 * <p>When constructed via the static factory {@link #create(String, int, int, RetryPolicy)},
 * this provider owns the CuratorFramework and will close it on {@link #close()}.
 * When constructed with an externally-provided CuratorFramework, the caller retains
 * ownership and is responsible for its lifecycle.</p>
 */
@Log4j2
public final class ZooKeeperCoordinationProvider implements CoordinationProvider {

    private final CuratorFramework client;
    private final boolean ownsClient;
    private final CopyOnWriteArrayList<ConnectionListener> connectionListeners;

    /**
     * Creates a provider wrapping an externally-managed CuratorFramework.
     * The caller retains ownership of the client and must close it separately.
     *
     * @param client an already-started CuratorFramework instance
     */
    public ZooKeeperCoordinationProvider(CuratorFramework client) {
        this(client, false);
    }

    /**
     * Constructor controlling ownership semantics. Package-private so that
     * {@link ZooKeeperProviderFactory} can pass an already-connected client
     * with ownership transfer.
     *
     * @param client     the CuratorFramework instance (must be started)
     * @param ownsClient if true, close() will also close the CuratorFramework
     */
    ZooKeeperCoordinationProvider(CuratorFramework client, boolean ownsClient) {
        this.client = Objects.requireNonNull(client, "client");
        this.ownsClient = ownsClient;
        this.connectionListeners = new CopyOnWriteArrayList<>();

        // Bridge Curator connection state events to our ConnectionListener API
        client.getConnectionStateListenable().addListener(
                (CuratorFramework c, ConnectionState newState) -> {
                    ConnectionListener.ConnectionState mapped = mapConnectionState(newState);
                    if (mapped != null) {
                        for (ConnectionListener listener : connectionListeners) {
                            try {
                                listener.onConnectionStateChange(mapped);
                            } catch (Exception e) {
                                log.error("Error notifying connection listener of state {}", mapped, e);
                            }
                        }
                    }
                }
        );
    }

    /**
     * Convenience factory that creates and starts a new CuratorFramework.
     * The returned provider owns the client and will close it on {@link #close()}.
     *
     * @param connectionString     ZooKeeper connection string (e.g. "localhost:2181")
     * @param sessionTimeoutMs     session timeout in milliseconds
     * @param connectionTimeoutMs  connection timeout in milliseconds
     * @param retryPolicy          the retry policy for transient failures
     * @return a fully connected provider
     */
    public static ZooKeeperCoordinationProvider create(String connectionString,
                                                        int sessionTimeoutMs,
                                                        int connectionTimeoutMs,
                                                        RetryPolicy retryPolicy) {
        CuratorFramework client = CuratorFrameworkFactory.builder()
                .connectString(connectionString)
                .sessionTimeoutMs(sessionTimeoutMs)
                .connectionTimeoutMs(connectionTimeoutMs)
                .retryPolicy(retryPolicy)
                .build();
        client.start();
        return new ZooKeeperCoordinationProvider(client, true);
    }

    // ---- Key-Value CRUD ----

    @Override
    public Optional<CoordinationEntry> get(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        try {
            Stat stat = client.checkExists().forPath(key);
            if (stat == null) {
                return Optional.empty();
            }
            Stat readStat = new Stat();
            byte[] data = client.getData().storingStatIn(readStat).forPath(key);
            return Optional.of(toEntry(key, data, readStat));
        } catch (KeeperException.NoNodeException e) {
            // Race: node deleted between checkExists and getData
            return Optional.empty();
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public long put(String key, byte[] value) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        try {
            Stat existing = client.checkExists().forPath(key);
            if (existing != null) {
                Stat stat = client.setData().forPath(key, value);
                return stat.getVersion() + 1L;
            }
            // Node does not exist -- create with parents
            try {
                client.create()
                        .creatingParentsIfNeeded()
                        .withMode(CreateMode.PERSISTENT)
                        .forPath(key, value);
                // After create, ZNode version is 0 -> offset to 1
                return 1;
            } catch (KeeperException.NodeExistsException e) {
                // Race: another writer created it between checkExists and create
                Stat stat = client.setData().forPath(key, value);
                return stat.getVersion() + 1L;
            }
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public long cas(String key, byte[] value, long expectedVersion) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        if (expectedVersion < 0) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "expectedVersion must be >= 0, got: " + expectedVersion);
        }
        if (expectedVersion > Integer.MAX_VALUE) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "expectedVersion exceeds ZooKeeper int range: " + expectedVersion);
        }
        try {
            if (expectedVersion == 0) {
                // Create-if-absent: the key must not exist
                client.create()
                        .creatingParentsIfNeeded()
                        .withMode(CreateMode.PERSISTENT)
                        .forPath(key, value);
                return 1;
            } else {
                // CAS update: subtract the +1 offset to get actual ZK version
                int zkVersion = (int) (expectedVersion - 1);
                Stat stat = client.setData()
                        .withVersion(zkVersion)
                        .forPath(key, value);
                return stat.getVersion() + 1L;
            }
        } catch (KeeperException.NodeExistsException e) {
            throw new CoordinationException(CoordinationException.Code.VERSION_CONFLICT,
                    "Key already exists (CAS with expectedVersion=0): " + key, e);
        } catch (KeeperException.BadVersionException e) {
            throw new CoordinationException(CoordinationException.Code.VERSION_CONFLICT,
                    "Version mismatch for key: " + key + ", expectedVersion=" + expectedVersion, e);
        } catch (KeeperException.NoNodeException e) {
            throw new CoordinationException(CoordinationException.Code.KEY_NOT_FOUND,
                    "Key not found for CAS update: " + key, e);
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public boolean delete(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        try {
            if (client.checkExists().forPath(key) == null) {
                return false;
            }
            client.delete().forPath(key);
            return true;
        } catch (KeeperException.NoNodeException e) {
            // Race: deleted between checkExists and delete
            return false;
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public void deleteTree(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        String normalizedKey = normalizePath(key);
        try {
            if (client.checkExists().forPath(normalizedKey) == null) {
                return;
            }
            client.delete().deletingChildrenIfNeeded().forPath(normalizedKey);
        } catch (KeeperException.NoNodeException e) {
            // Already gone -- idempotent
            log.debug("Node {} already deleted during deleteTree", normalizedKey);
        } catch (Exception e) {
            throw mapException(e, normalizedKey);
        }
    }

    @Override
    public List<CoordinationEntry> list(String prefix) throws CoordinationException {
        Objects.requireNonNull(prefix, "prefix");
        String normalizedPrefix = normalizePath(prefix);
        try {
            Stat stat = client.checkExists().forPath(normalizedPrefix);
            if (stat == null) {
                return Collections.emptyList();
            }
            List<String> children = client.getChildren().forPath(normalizedPrefix);
            if (children.isEmpty()) {
                return Collections.emptyList();
            }
            List<CoordinationEntry> entries = new ArrayList<>(children.size());
            for (String child : children) {
                String childPath = normalizedPrefix.endsWith("/")
                        ? normalizedPrefix + child
                        : normalizedPrefix + "/" + child;
                try {
                    Stat childStat = new Stat();
                    byte[] data = client.getData().storingStatIn(childStat).forPath(childPath);
                    entries.add(toEntry(childPath, data, childStat));
                } catch (KeeperException.NoNodeException e) {
                    // Child was deleted concurrently -- skip
                    log.debug("Child node {} disappeared during list, skipping", childPath);
                }
            }
            return entries;
        } catch (Exception e) {
            throw mapException(e, normalizedPrefix);
        }
    }

    @Override
    public List<CoordinationEntry> listRecursive(String prefix) throws CoordinationException {
        Objects.requireNonNull(prefix, "prefix");
        String normalizedPrefix = normalizePath(prefix);
        try {
            Stat stat = client.checkExists().forPath(normalizedPrefix);
            if (stat == null) {
                return Collections.emptyList();
            }
            List<CoordinationEntry> entries = new ArrayList<>();
            collectChildrenRecursive(normalizedPrefix, entries);
            return entries;
        } catch (Exception e) {
            throw mapException(e, normalizedPrefix);
        }
    }

    // ---- Ephemeral and Sequential keys ----

    @Override
    public long putEphemeral(String key, byte[] value) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        try {
            // Ephemeral nodes cannot have children in ZK, so we do not use
            // creatingParentsIfNeeded with EPHEMERAL mode (Curator handles this
            // by creating persistent parents and an ephemeral leaf).
            client.create()
                    .creatingParentsIfNeeded()
                    .withMode(CreateMode.EPHEMERAL)
                    .forPath(key, value);
            return 1;
        } catch (KeeperException.NodeExistsException e) {
            // Ephemeral node already exists (stale session?) -- overwrite it
            try {
                Stat stat = client.setData().forPath(key, value);
                return stat.getVersion() + 1L;
            } catch (Exception ex) {
                throw mapException(ex, key);
            }
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public String putSequential(String keyPrefix, byte[] value) throws CoordinationException {
        Objects.requireNonNull(keyPrefix, "keyPrefix");
        Objects.requireNonNull(value, "value");
        try {
            return client.create()
                    .creatingParentsIfNeeded()
                    .withMode(CreateMode.PERSISTENT_SEQUENTIAL)
                    .forPath(keyPrefix, value);
        } catch (Exception e) {
            throw mapException(e, keyPrefix);
        }
    }

    // ---- Watch ----

    @Override
    public Closeable watch(String keyOrPrefix, Consumer<WatchEvent> listener) throws CoordinationException {
        Objects.requireNonNull(keyOrPrefix, "keyOrPrefix");
        Objects.requireNonNull(listener, "listener");
        String watchPath = normalizePath(keyOrPrefix);
        try {
            CuratorCache cache = CuratorCache.build(client, watchPath);

            CuratorCacheListener cacheListener = CuratorCacheListener.builder()
                    .forCreates(childData -> {
                        try {
                            CoordinationEntry entry = toEntry(
                                    childData.getPath(), childData.getData(), childData.getStat());
                            listener.accept(new WatchEvent(WatchEvent.Type.PUT, entry, null));
                        } catch (Exception e) {
                            log.error("Error dispatching watch CREATE event for path {}",
                                    childData.getPath(), e);
                        }
                    })
                    .forChanges((oldData, newData) -> {
                        try {
                            CoordinationEntry previous = toEntry(
                                    oldData.getPath(), oldData.getData(), oldData.getStat());
                            CoordinationEntry current = toEntry(
                                    newData.getPath(), newData.getData(), newData.getStat());
                            listener.accept(new WatchEvent(WatchEvent.Type.PUT, current, previous));
                        } catch (Exception e) {
                            log.error("Error dispatching watch CHANGE event for path {}",
                                    newData.getPath(), e);
                        }
                    })
                    .forDeletes(oldData -> {
                        try {
                            CoordinationEntry previous = toEntry(
                                    oldData.getPath(), oldData.getData(), oldData.getStat());
                            listener.accept(new WatchEvent(WatchEvent.Type.DELETE, null, previous));
                        } catch (Exception e) {
                            log.error("Error dispatching watch DELETE event for path {}",
                                    oldData.getPath(), e);
                        }
                    })
                    .build();

            cache.listenable().addListener(cacheListener);
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

    // ---- Leader election ----

    @Override
    public LeaderElection leaderElection(String path, String participantId) throws CoordinationException {
        Objects.requireNonNull(path, "path");
        Objects.requireNonNull(participantId, "participantId");
        try {
            LeaderLatch latch = new LeaderLatch(client, path, participantId);
            return new ZooKeeperLeaderElection(latch, path, participantId);
        } catch (Exception e) {
            throw mapException(e, path);
        }
    }

    // ---- Distributed locking ----

    @Override
    public DistributedLock acquireLock(String lockPath, long timeout, TimeUnit unit)
            throws CoordinationException {
        Objects.requireNonNull(lockPath, "lockPath");
        Objects.requireNonNull(unit, "unit");
        try {
            InterProcessMutex mutex = new InterProcessMutex(client, lockPath);
            if (!mutex.acquire(timeout, unit)) {
                throw new CoordinationException(CoordinationException.Code.LOCK_ACQUISITION_FAILED,
                        "Failed to acquire lock within " + timeout + " " + unit + ": " + lockPath);
            }
            log.debug("Acquired lock on {}", lockPath);
            return new ZooKeeperDistributedLock(mutex, lockPath);
        } catch (CoordinationException e) {
            throw e;
        } catch (Exception e) {
            throw mapException(e, lockPath);
        }
    }

    // ---- Connection health ----

    @Override
    public boolean isConnected() {
        return client.getZookeeperClient().isConnected();
    }

    @Override
    public void addConnectionListener(ConnectionListener listener) {
        Objects.requireNonNull(listener, "listener");
        connectionListeners.add(listener);
    }

    // ---- Lifecycle ----

    @Override
    public void close() throws IOException {
        log.debug("Closing ZooKeeperCoordinationProvider (ownsClient={})", ownsClient);
        if (ownsClient) {
            client.close();
        }
    }

    // ---- Internal helpers ----

    /**
     * ZooKeeper versions start at 0, but the CoordinationProvider contract reserves
     * version 0 for create-if-absent semantics. We offset by +1 so returned versions
     * are always >= 1.
     */
    private static CoordinationEntry toEntry(String path, byte[] data, Stat stat) {
        return new CoordinationEntry(
                path,
                data != null ? data : new byte[0],
                stat.getVersion() + 1L,
                stat.getCzxid()
        );
    }

    /**
     * Normalizes a key path by stripping trailing slash (ZK rejects paths ending with '/').
     */
    private static String normalizePath(String path) {
        if (path.length() > 1 && path.endsWith("/")) {
            return path.substring(0, path.length() - 1);
        }
        return path;
    }

    /**
     * Recursively collects all descendant nodes under the given path.
     */
    private void collectChildrenRecursive(String path, List<CoordinationEntry> entries)
            throws Exception {
        List<String> children;
        try {
            children = client.getChildren().forPath(path);
        } catch (KeeperException.NoNodeException e) {
            return;
        }
        for (String child : children) {
            String childPath = path.endsWith("/") ? path + child : path + "/" + child;
            try {
                Stat childStat = new Stat();
                byte[] data = client.getData().storingStatIn(childStat).forPath(childPath);
                entries.add(toEntry(childPath, data, childStat));
                collectChildrenRecursive(childPath, entries);
            } catch (KeeperException.NoNodeException e) {
                log.debug("Child node {} disappeared during listRecursive, skipping", childPath);
            }
        }
    }

    /**
     * Maps ZooKeeper/Curator exceptions to {@link CoordinationException} with appropriate codes.
     */
    private static CoordinationException mapException(Exception e, String key) {
        return switch (e) {
            case KeeperException.NoNodeException _ ->
                    new CoordinationException(CoordinationException.Code.KEY_NOT_FOUND,
                            "Key not found: " + key, e);
            case KeeperException.NodeExistsException _ ->
                    new CoordinationException(CoordinationException.Code.KEY_EXISTS,
                            "Key already exists: " + key, e);
            case KeeperException.BadVersionException _ ->
                    new CoordinationException(CoordinationException.Code.VERSION_CONFLICT,
                            "Version conflict for key: " + key, e);
            case KeeperException.ConnectionLossException _, KeeperException.SessionExpiredException _ ->
                    new CoordinationException(CoordinationException.Code.CONNECTION_LOST,
                            "Connection lost while accessing key: " + key, e);
            case KeeperException.OperationTimeoutException _ ->
                    new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                            "Operation timed out for key: " + key, e);
            case InterruptedException _ -> {
                Thread.currentThread().interrupt();
                yield new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                        "Operation interrupted for key: " + key, e);
            }
            default -> new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Internal error for key: " + key, e);
        };
    }

    /**
     * Maps Curator's {@link ConnectionState} to our {@link ConnectionListener.ConnectionState}.
     */
    private static ConnectionListener.ConnectionState mapConnectionState(ConnectionState curatorState) {
        return switch (curatorState) {
            case CONNECTED -> ConnectionListener.ConnectionState.CONNECTED;
            case SUSPENDED -> ConnectionListener.ConnectionState.SUSPENDED;
            case LOST -> ConnectionListener.ConnectionState.LOST;
            case RECONNECTED -> ConnectionListener.ConnectionState.RECONNECTED;
            case READ_ONLY -> {
                // ZK read-only mode: treat as suspended since writes will fail
                log.warn("ZooKeeper entered read-only mode, treating as SUSPENDED");
                yield ConnectionListener.ConnectionState.SUSPENDED;
            }
        };
    }
}
