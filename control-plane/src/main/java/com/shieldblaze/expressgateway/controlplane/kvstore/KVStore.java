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
package com.shieldblaze.expressgateway.controlplane.kvstore;

import java.io.Closeable;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * Pluggable key-value store abstraction for the ExpressGateway control plane.
 *
 * <p>Implementations must be thread-safe. Keys are hierarchical path strings
 * (e.g. {@code /expressgateway/config/clusters/cluster-1}). Values are opaque byte arrays.</p>
 *
 * <p>Version semantics: every mutation increments a version counter for the key.
 * The returned version from {@link #put} and {@link #cas} is the new version after the write.
 * {@link #cas} will fail with {@link KVStoreException.Code#VERSION_CONFLICT} if the key's
 * current version does not match {@code expectedVersion}.</p>
 */
public interface KVStore extends Closeable {

    /**
     * Retrieve a single key.
     *
     * @param key The key to retrieve
     * @return The entry if it exists, or {@link Optional#empty()} if not found
     * @throws KVStoreException On backend errors
     */
    Optional<KVEntry> get(String key) throws KVStoreException;

    /**
     * List all direct children under the given prefix.
     *
     * @param prefix The prefix path whose children to list
     * @return A list of entries (may be empty)
     * @throws KVStoreException On backend errors
     */
    List<KVEntry> list(String prefix) throws KVStoreException;

    /**
     * List all descendants under the given prefix, recursively traversing all
     * levels of the key hierarchy.
     *
     * @param prefix The prefix path whose descendants to list
     * @return A list of all entries at any depth under the prefix (may be empty)
     * @throws KVStoreException On backend errors
     */
    List<KVEntry> listRecursive(String prefix) throws KVStoreException;

    /**
     * Unconditionally write a value for the given key, creating it if it does not exist.
     *
     * @param key   The key to write
     * @param value The value bytes
     * @return The new version after the write
     * @throws KVStoreException On backend errors
     */
    long put(String key, byte[] value) throws KVStoreException;

    /**
     * Compare-and-swap: write the value only if the key's current version matches
     * {@code expectedVersion}. If {@code expectedVersion == 0}, the key must not exist
     * (create-if-absent semantics).
     *
     * @param key             The key to write
     * @param value           The value bytes
     * @param expectedVersion The version that the key must currently be at
     * @return The new version after the write
     * @throws KVStoreException With {@link KVStoreException.Code#VERSION_CONFLICT} on mismatch
     */
    long cas(String key, byte[] value, long expectedVersion) throws KVStoreException;

    /**
     * Delete a single key.
     *
     * @param key The key to delete
     * @return {@code true} if the key existed and was deleted, {@code false} if it did not exist
     * @throws KVStoreException On backend errors
     */
    boolean delete(String key) throws KVStoreException;

    /**
     * Recursively delete a key and all of its children.
     *
     * @param key The root key to delete
     * @throws KVStoreException On backend errors
     */
    void deleteTree(String key) throws KVStoreException;

    /**
     * Install a persistent watch on a key or prefix. The returned {@link Closeable} cancels
     * the watch when closed.
     *
     * @param keyOrPrefix The key or prefix to watch
     * @param watcher     The callback to invoke on mutations
     * @return A handle that cancels the watch when closed
     * @throws KVStoreException On backend errors
     */
    Closeable watch(String keyOrPrefix, KVWatcher watcher) throws KVStoreException;

    /**
     * Acquire a distributed lock on the given path. The returned {@link Closeable} releases
     * the lock when closed.
     *
     * <p>This call blocks until the lock is acquired.</p>
     *
     * @param lockPath The path to use for the lock
     * @return A handle that releases the lock when closed
     * @throws KVStoreException On backend errors or if the lock cannot be acquired
     */
    Closeable acquireLock(String lockPath) throws KVStoreException;

    /**
     * Start a leader election on the given path. The returned {@link LeaderElection} allows
     * the caller to check leadership status and register listeners for leadership changes.
     *
     * @param electionPath  The path to use for election
     * @param participantId A unique identifier for this participant
     * @return A {@link LeaderElection} handle
     * @throws KVStoreException On backend errors
     */
    LeaderElection leaderElection(String electionPath, String participantId) throws KVStoreException;

    // ---- Batch and transaction operations (default implementations for backward compatibility) ----

    /**
     * Atomically writes multiple key-value pairs. Either all writes succeed or none do.
     *
     * <p>Default implementation performs individual puts sequentially. Backends that
     * support native transactions (etcd multi-key txn, ZooKeeper multi) should
     * override this for true atomicity.</p>
     *
     * @param entries map of key to value bytes to write atomically
     * @throws KVStoreException if any write fails (partial state may exist with default impl)
     */
    default void batchPut(Map<String, byte[]> entries) throws KVStoreException {
        for (Map.Entry<String, byte[]> entry : entries.entrySet()) {
            put(entry.getKey(), entry.getValue());
        }
    }

    /**
     * Atomically reads multiple keys.
     *
     * <p>Default implementation reads keys sequentially. Backends that support
     * multi-key reads should override for consistency and performance.</p>
     *
     * @param keys the keys to read
     * @return list of entries found (entries for missing keys are omitted)
     * @throws KVStoreException on backend errors
     */
    default List<KVEntry> batchGet(List<String> keys) throws KVStoreException {
        List<KVEntry> results = new ArrayList<>(keys.size());
        for (String key : keys) {
            get(key).ifPresent(results::add);
        }
        return results;
    }

    /**
     * Executes a compare-and-swap transaction: atomically checks that all keys
     * match their expected versions, then applies all writes.
     *
     * <p>Default implementation uses individual CAS operations. This does NOT provide
     * true atomicity across keys; backends with native transaction support (etcd, ZooKeeper multi)
     * should override for true multi-key CAS.</p>
     *
     * @param expectedVersions map of key to its expected current version (0 = must not exist)
     * @param writes           map of key to new value bytes to write
     * @throws KVStoreException with {@link KVStoreException.Code#VERSION_CONFLICT} if any
     *                          expected version does not match
     */
    default void casTransaction(Map<String, Long> expectedVersions, Map<String, byte[]> writes)
            throws KVStoreException {
        // Verify all versions first
        for (Map.Entry<String, Long> entry : expectedVersions.entrySet()) {
            String key = entry.getKey();
            long expectedVersion = entry.getValue();
            Optional<KVEntry> current = get(key);

            if (expectedVersion == 0) {
                if (current.isPresent()) {
                    throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                            "Key already exists (expected absent): " + key);
                }
            } else {
                if (current.isEmpty()) {
                    throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                            "Key does not exist (expected version " + expectedVersion + "): " + key);
                }
                if (current.get().version() != expectedVersion) {
                    throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                            "Version mismatch for key " + key + ": expected=" + expectedVersion +
                                    ", actual=" + current.get().version());
                }
            }
        }
        // Apply writes
        for (Map.Entry<String, byte[]> write : writes.entrySet()) {
            put(write.getKey(), write.getValue());
        }
    }

    /**
     * Leader election handle. Closing this handle withdraws from the election.
     */
    interface LeaderElection extends Closeable {

        /**
         * @return {@code true} if this participant currently holds leadership
         */
        boolean isLeader();

        /**
         * Returns the participant ID of the current leader.
         *
         * @return The current leader's participant ID
         * @throws KVStoreException If the leader cannot be determined (e.g., no leader elected,
         *                          connection lost, or the backend does not support this query)
         */
        String currentLeaderId() throws KVStoreException;

        /**
         * Register a listener that is called when leadership status changes.
         *
         * @param listener The listener to add
         */
        void addListener(LeaderChangeListener listener);
    }

    /**
     * Listener for leader election state transitions.
     */
    @FunctionalInterface
    interface LeaderChangeListener {

        /**
         * Called when this participant's leadership status changes.
         *
         * @param isLeader {@code true} if this participant became the leader,
         *                 {@code false} if leadership was lost
         */
        void onLeaderChange(boolean isLeader);
    }
}
