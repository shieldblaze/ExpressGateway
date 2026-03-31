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
package com.shieldblaze.expressgateway.configuration.distributed;

import java.io.Closeable;
import java.util.List;
import java.util.Optional;

/**
 * Storage abstraction for the distributed configuration subsystem.
 *
 * <p>This interface decouples the configuration classes from any specific backend
 * (ZooKeeper/Curator, etcd, Consul, etc.). Implementations must be thread-safe.</p>
 *
 * <p>Keys are hierarchical path strings (e.g. {@code /ExpressGateway/prod/cluster-1/config/versions/v001}).</p>
 */
public interface ConfigStorageBackend {

    /**
     * Read data at the given key.
     *
     * @param key The key to read
     * @return The data bytes if the key exists, or {@link Optional#empty()} if not found
     * @throws Exception On backend errors
     */
    Optional<byte[]> get(String key) throws Exception;

    /**
     * Create or update data at the given key. Parent paths are created if needed.
     *
     * @param key   The key to write
     * @param value The value bytes
     * @throws Exception On backend errors
     */
    void put(String key, byte[] value) throws Exception;

    /**
     * Create a new key with data. Fails if the key already exists.
     *
     * @param key   The key to create
     * @param value The value bytes
     * @throws KeyExistsException If the key already exists
     * @throws Exception          On backend errors
     */
    void putIfAbsent(String key, byte[] value) throws Exception;

    /**
     * Create a persistent sequential node under the given prefix. The backend appends a
     * monotonically increasing sequence number to the prefix.
     *
     * @param keyPrefix The key prefix (e.g. {@code /path/to/entry-})
     * @param value     The value bytes
     * @return The full path of the created node (including the sequence suffix)
     * @throws Exception On backend errors
     */
    String putSequential(String keyPrefix, byte[] value) throws Exception;

    /**
     * Create an ephemeral key that is automatically deleted when this client's session ends.
     * This is used for quorum ACK nodes that must be cleaned up on node disconnect.
     *
     * @param key   The key to create
     * @param value The value bytes
     * @throws KeyExistsException If the key already exists (idempotent ACK)
     * @throws Exception          On backend errors
     */
    void putEphemeral(String key, byte[] value) throws Exception;

    /**
     * Check if a key exists.
     *
     * @param key The key to check
     * @return {@code true} if the key exists
     * @throws Exception On backend errors
     */
    boolean exists(String key) throws Exception;

    /**
     * List the names of direct children under the given key.
     *
     * @param key The parent key
     * @return A list of child names (not full paths), or an empty list
     * @throws Exception On backend errors
     */
    List<String> listChildren(String key) throws Exception;

    /**
     * Delete a single key.
     *
     * @param key The key to delete
     * @throws Exception On backend errors
     */
    void delete(String key) throws Exception;

    /**
     * Delete a key and all of its children recursively.
     *
     * @param key The root key to delete
     * @throws Exception On backend errors
     */
    void deleteTree(String key) throws Exception;

    /**
     * Install a watch on the given key or prefix. The {@link WatchListener} is called
     * when data is created or changed at the watched path.
     *
     * @param path     The key or prefix to watch
     * @param listener The callback for change notifications
     * @return A handle that cancels the watch when closed
     * @throws Exception On backend errors
     */
    Closeable watch(String path, WatchListener listener) throws Exception;

    /**
     * Start a leader election on the given path.
     *
     * @param electionPath  The path to use for election
     * @param participantId A unique identifier for this participant
     * @return A {@link LeaderElection} handle
     * @throws Exception On backend errors
     */
    LeaderElection leaderElection(String electionPath, String participantId) throws Exception;

    /**
     * Register a listener that is called when the backend connection is lost or suspended.
     * This is used by the quorum manager to fail active waits on session loss.
     *
     * @param listener The listener to invoke on connection loss
     */
    void addConnectionLossListener(Runnable listener);

    /**
     * Remove a previously registered connection loss listener.
     *
     * @param listener The listener to remove
     */
    void removeConnectionLossListener(Runnable listener);

    // --- Nested types ---

    /**
     * Callback interface for watch events.
     */
    interface WatchListener {
        /**
         * Called when data at the watched path is created or changed.
         *
         * @param data The new data bytes (may be null or empty)
         */
        void onDataChanged(byte[] data);
    }

    /**
     * Leader election handle. Closing this handle withdraws from the election.
     */
    interface LeaderElection extends Closeable {

        /**
         * Start participating in the election.
         *
         * @throws Exception On backend errors
         */
        void start() throws Exception;

        /**
         * @return {@code true} if this participant currently holds leadership
         */
        boolean isLeader();

        /**
         * Register a listener for leadership state changes.
         *
         * @param listener Called with {@code true} when leadership is gained,
         *                 {@code false} when lost
         */
        void addListener(java.util.function.Consumer<Boolean> listener);
    }

    /**
     * Thrown when a key already exists and the operation requires it to be absent.
     */
    class KeyExistsException extends Exception {
        public KeyExistsException(String key) {
            super("Key already exists: " + key);
        }

        public KeyExistsException(String key, Throwable cause) {
            super("Key already exists: " + key, cause);
        }
    }
}
