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
package com.shieldblaze.expressgateway.coordination;

import java.io.Closeable;
import java.io.IOException;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;

/**
 * Backend-agnostic coordination provider for distributed key-value storage,
 * leader election, distributed locking, and watch notifications.
 *
 * <p>This is the single integration point for all coordination needs in
 * ExpressGateway. Implementations exist for ZooKeeper, etcd, and Consul.
 * The API surface is intentionally free of any backend-specific types.</p>
 *
 * <h2>Key format</h2>
 * <p>Keys are hierarchical path strings using {@code /} as separator
 * (e.g. {@code /expressgateway/config/clusters/cluster-1}).
 * Implementations MUST normalize trailing slashes.</p>
 *
 * <h2>Version semantics</h2>
 * <p>Every mutation increments a per-key version counter. The returned version
 * from {@link #put} and {@link #cas} is the version after the write.
 * Version 0 is reserved for create-if-absent semantics in CAS.
 * Existing keys always have version >= 1.</p>
 *
 * <h2>Thread safety</h2>
 * <p>All implementations MUST be thread-safe. Watch and listener callbacks may
 * fire from backend-internal threads.</p>
 */
public interface CoordinationProvider extends Closeable {

    // ---- Key-Value CRUD ----

    /**
     * Retrieve a single key.
     *
     * @param key the key to retrieve
     * @return the entry if it exists, or {@link Optional#empty()} if not found
     * @throws CoordinationException on backend errors
     */
    Optional<CoordinationEntry> get(String key) throws CoordinationException;

    /**
     * Unconditionally write a value for the given key, creating it if it does not exist.
     * If the key exists, its value is overwritten and version incremented.
     *
     * @param key   the key to write
     * @param value the value bytes
     * @return the new version after the write (always >= 1)
     * @throws CoordinationException on backend errors
     */
    long put(String key, byte[] value) throws CoordinationException;

    /**
     * Compare-and-swap: write the value only if the key's current version matches
     * {@code expectedVersion}. If {@code expectedVersion == 0}, the key must not
     * exist (create-if-absent semantics).
     *
     * @param key             the key to write
     * @param value           the value bytes
     * @param expectedVersion the version that the key must currently be at
     * @return the new version after the write
     * @throws CoordinationException with {@link CoordinationException.Code#VERSION_CONFLICT}
     *                               on version mismatch, {@link CoordinationException.Code#KEY_NOT_FOUND}
     *                               if the key does not exist and expectedVersion != 0
     */
    long cas(String key, byte[] value, long expectedVersion) throws CoordinationException;

    /**
     * Delete a single key (leaf node only).
     *
     * @param key the key to delete
     * @return {@code true} if the key existed and was deleted, {@code false} if it did not exist
     * @throws CoordinationException on backend errors
     */
    boolean delete(String key) throws CoordinationException;

    /**
     * Recursively delete a key and all of its children. Idempotent: no error if
     * the key does not exist.
     *
     * @param key the root key to delete
     * @throws CoordinationException on backend errors
     */
    void deleteTree(String key) throws CoordinationException;

    /**
     * List all direct children under the given prefix.
     *
     * @param prefix the prefix path whose children to list
     * @return a list of entries (may be empty, never null)
     * @throws CoordinationException on backend errors
     */
    List<CoordinationEntry> list(String prefix) throws CoordinationException;

    /**
     * List all descendants under the given prefix, recursively traversing all
     * levels of the key hierarchy.
     *
     * @param prefix the prefix path whose descendants to list
     * @return a list of all entries at any depth under the prefix (may be empty)
     * @throws CoordinationException on backend errors
     */
    List<CoordinationEntry> listRecursive(String prefix) throws CoordinationException;

    // ---- Ephemeral and Sequential keys ----

    /**
     * Write an ephemeral (session-bound) key. The key is automatically deleted when
     * this provider's session ends (e.g. ZK session expiry, etcd lease expiry).
     *
     * @param key   the key to write
     * @param value the value bytes
     * @return the new version after the write
     * @throws CoordinationException on backend errors
     */
    long putEphemeral(String key, byte[] value) throws CoordinationException;

    /**
     * Create a sequential key with a monotonically increasing suffix appended to
     * the given prefix. Useful for ordered queues and barriers.
     *
     * @param keyPrefix the prefix for the sequential key (e.g. {@code /queue/item-})
     * @param value     the value bytes
     * @return the full path of the created key including the generated suffix
     * @throws CoordinationException on backend errors
     */
    String putSequential(String keyPrefix, byte[] value) throws CoordinationException;

    // ---- Watch ----

    /**
     * Install a persistent watch on a key or prefix. The watch fires for all
     * mutations under the given path (including child nodes). The returned
     * {@link Closeable} cancels the watch when closed.
     *
     * <p>The consumer is called from a backend-internal thread. Implementations
     * MUST NOT block the consumer callback.</p>
     *
     * @param keyOrPrefix the key or prefix to watch
     * @param listener    the callback to invoke on mutations
     * @return a handle that cancels the watch when closed
     * @throws CoordinationException on backend errors
     */
    Closeable watch(String keyOrPrefix, Consumer<WatchEvent> listener) throws CoordinationException;

    // ---- Leader election ----

    /**
     * Create a leader election instance for the given path. The election does NOT
     * start until {@link LeaderElection#start()} is called on the returned handle.
     *
     * @param path          the election path (e.g. {@code /expressgateway/leader})
     * @param participantId a unique identifier for this participant
     * @return a {@link LeaderElection} handle (not yet started)
     * @throws CoordinationException on backend errors during setup
     */
    LeaderElection leaderElection(String path, String participantId) throws CoordinationException;

    // ---- Distributed locking ----

    /**
     * Acquire a distributed lock on the given path. This call blocks until the lock
     * is acquired or the timeout expires.
     *
     * @param lockPath the path to use for the lock
     * @param timeout  the maximum time to wait for the lock
     * @param unit     the time unit for the timeout
     * @return a {@link DistributedLock} handle; call {@link DistributedLock#release()} when done
     * @throws CoordinationException with {@link CoordinationException.Code#LOCK_ACQUISITION_FAILED}
     *                               if the lock cannot be acquired within the timeout,
     *                               or {@link CoordinationException.Code#OPERATION_TIMEOUT}
     *                               if the operation times out at the transport level
     */
    DistributedLock acquireLock(String lockPath, long timeout, TimeUnit unit) throws CoordinationException;

    // ---- Connection health ----

    /**
     * Returns whether the provider is currently connected to the backend.
     *
     * @return {@code true} if connected and operational
     */
    boolean isConnected();

    /**
     * Registers a listener for connection state changes.
     *
     * @param listener the listener to add; must not be null
     */
    void addConnectionListener(ConnectionListener listener);

    // ---- Batch operations (default implementations) ----

    /**
     * Write multiple key-value pairs. Default implementation performs sequential puts.
     * Backends with native batch/transaction support should override for atomicity.
     *
     * @param entries map of key to value bytes
     * @throws CoordinationException if any write fails
     */
    default void batchPut(Map<String, byte[]> entries) throws CoordinationException {
        for (Map.Entry<String, byte[]> entry : entries.entrySet()) {
            put(entry.getKey(), entry.getValue());
        }
    }

    /**
     * Read multiple keys. Default implementation reads sequentially.
     * Backends with native multi-key reads should override for consistency.
     *
     * @param keys the keys to read
     * @return list of entries found (missing keys are omitted)
     * @throws CoordinationException on backend errors
     */
    default List<CoordinationEntry> batchGet(List<String> keys) throws CoordinationException {
        List<CoordinationEntry> results = new ArrayList<>(keys.size());
        for (String key : keys) {
            get(key).ifPresent(results::add);
        }
        return results;
    }

    /**
     * Closes this provider and releases all resources (connections, caches, threads).
     *
     * @throws IOException if an error occurs during shutdown
     */
    @Override
    void close() throws IOException;
}
