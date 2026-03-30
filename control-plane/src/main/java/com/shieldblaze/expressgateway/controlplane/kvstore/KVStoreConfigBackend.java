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

import com.shieldblaze.expressgateway.configuration.distributed.ConfigStorageBackend;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * Adapts a {@link KVStore} to the {@link ConfigStorageBackend} interface used by
 * the distributed configuration subsystem.
 *
 * <p>This adapter bridges the control-plane KVStore abstraction into the configuration
 * module, eliminating direct Curator/ZooKeeper dependencies from the config classes.</p>
 */
public final class KVStoreConfigBackend implements ConfigStorageBackend {

    private static final Logger logger = LogManager.getLogger(KVStoreConfigBackend.class);

    private final KVStore kvStore;
    private final List<Runnable> connectionLossListeners = new CopyOnWriteArrayList<>();

    public KVStoreConfigBackend(KVStore kvStore) {
        this.kvStore = Objects.requireNonNull(kvStore, "kvStore");
    }

    @Override
    public Optional<byte[]> get(String key) throws Exception {
        try {
            Optional<KVEntry> entry = kvStore.get(key);
            return entry.map(KVEntry::value);
        } catch (KVStoreException e) {
            if (e.code() == KVStoreException.Code.KEY_NOT_FOUND) {
                return Optional.empty();
            }
            throw e;
        }
    }

    @Override
    public void put(String key, byte[] value) throws Exception {
        kvStore.put(key, value);
    }

    @Override
    public void putIfAbsent(String key, byte[] value) throws Exception {
        try {
            kvStore.cas(key, value, 0);
        } catch (KVStoreException e) {
            if (e.code() == KVStoreException.Code.VERSION_CONFLICT) {
                throw new KeyExistsException(key, e);
            }
            throw e;
        }
    }

    @Override
    public String putSequential(String keyPrefix, byte[] value) throws Exception {
        // Sequential nodes require the parent to exist. Use a two-step approach:
        // extract the parent path, ensure it exists, then create the sequential child.
        // The KVStore interface doesn't directly support sequential nodes, so we
        // implement a simple counter-based approach using list + cas.
        String parentPath = keyPrefix.substring(0, keyPrefix.lastIndexOf('/'));
        String prefix = keyPrefix.substring(keyPrefix.lastIndexOf('/') + 1);

        // Ensure parent exists
        if (kvStore.get(parentPath).isEmpty()) {
            kvStore.put(parentPath, new byte[0]);
        }

        // List existing children to find the next sequence number
        List<KVEntry> children = kvStore.list(parentPath);
        long maxSeq = 0;
        for (KVEntry child : children) {
            String childName = child.key().substring(child.key().lastIndexOf('/') + 1);
            if (childName.startsWith(prefix)) {
                String seqStr = childName.substring(prefix.length());
                try {
                    long seq = Long.parseLong(seqStr);
                    if (seq > maxSeq) {
                        maxSeq = seq;
                    }
                } catch (NumberFormatException ignored) {
                }
            }
        }

        String seqKey = parentPath + "/" + prefix + String.format("%010d", maxSeq + 1);
        kvStore.put(seqKey, value);
        return seqKey;
    }

    @Override
    public void putEphemeral(String key, byte[] value) throws Exception {
        // KVStore doesn't have a native ephemeral concept.
        // For backends that support it (ZK), the ZooKeeperKVStore implementation handles
        // session-scoped cleanup. For other backends, we use a regular put.
        // The quorum manager's connection loss listener handles cleanup semantics.
        try {
            kvStore.cas(key, value, 0); // create-if-absent
        } catch (KVStoreException e) {
            if (e.code() == KVStoreException.Code.VERSION_CONFLICT) {
                throw new KeyExistsException(key, e);
            }
            throw e;
        }
    }

    @Override
    public boolean exists(String key) throws Exception {
        return kvStore.get(key).isPresent();
    }

    @Override
    public List<String> listChildren(String key) throws Exception {
        List<KVEntry> entries = kvStore.list(key);
        List<String> names = new ArrayList<>(entries.size());
        for (KVEntry entry : entries) {
            String fullPath = entry.key();
            String childName = fullPath.substring(fullPath.lastIndexOf('/') + 1);
            names.add(childName);
        }
        return names;
    }

    @Override
    public void delete(String key) throws Exception {
        kvStore.delete(key);
    }

    @Override
    public void deleteTree(String key) throws Exception {
        kvStore.deleteTree(key);
    }

    @Override
    public Closeable watch(String path, WatchListener listener) throws Exception {
        return kvStore.watch(path, event -> {
            if (event.type() == KVWatchEvent.Type.PUT && event.entry() != null) {
                listener.onDataChanged(event.entry().value());
            }
        });
    }

    @Override
    public LeaderElection leaderElection(String electionPath, String participantId) throws Exception {
        KVStore.LeaderElection kvElection = kvStore.leaderElection(electionPath, participantId);

        return new LeaderElection() {
            @Override
            public void start() throws Exception {
                // KVStore.leaderElection already starts the election
            }

            @Override
            public boolean isLeader() {
                return kvElection.isLeader();
            }

            @Override
            public void addListener(java.util.function.Consumer<Boolean> listener) {
                kvElection.addListener(listener::accept);
            }

            @Override
            public void close() throws IOException {
                kvElection.close();
            }
        };
    }

    @Override
    public void addConnectionLossListener(Runnable listener) {
        connectionLossListeners.add(listener);
    }

    @Override
    public void removeConnectionLossListener(Runnable listener) {
        connectionLossListeners.remove(listener);
    }

    /**
     * Notify all registered connection loss listeners. This should be called by the
     * infrastructure layer when a backend connection loss is detected.
     */
    public void notifyConnectionLoss() {
        for (Runnable listener : connectionLossListeners) {
            try {
                listener.run();
            } catch (Exception e) {
                logger.warn("Connection loss listener threw exception", e);
            }
        }
    }
}
