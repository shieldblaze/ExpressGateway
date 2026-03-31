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
package com.shieldblaze.expressgateway.controlplane.testutil;

import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatcher;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatchEvent;

import java.io.Closeable;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

/**
 * In-memory KVStore implementation for unit tests that do not need a real backend.
 *
 * <p>Supports watches with fire-on-put/delete semantics, and a controllable failure
 * mode via {@link #setFailOnWrite(boolean)} for chaos testing.</p>
 *
 * <p>Thread-safe: all operations delegate to {@link ConcurrentHashMap}.</p>
 */
public class InMemoryKVStore implements KVStore {

    private final ConcurrentHashMap<String, byte[]> store = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, Long> versions = new ConcurrentHashMap<>();
    private final AtomicLong version = new AtomicLong(0);
    private final CopyOnWriteArrayList<WatchRegistration> watches = new CopyOnWriteArrayList<>();
    private final AtomicBoolean failOnWrite = new AtomicBoolean(false);

    /**
     * When set to true, all put/cas operations will throw KVStoreException.
     */
    public void setFailOnWrite(boolean fail) {
        failOnWrite.set(fail);
    }

    public boolean isFailOnWrite() {
        return failOnWrite.get();
    }

    @Override
    public Optional<KVEntry> get(String key) throws KVStoreException {
        byte[] v = store.get(key);
        if (v == null) {
            return Optional.empty();
        }
        Long ver = versions.getOrDefault(key, 1L);
        return Optional.of(new KVEntry(key, v, ver, 1));
    }

    @Override
    public List<KVEntry> list(String prefix) throws KVStoreException {
        String normalizedPrefix = prefix.endsWith("/") ? prefix : prefix + "/";
        return store.entrySet().stream()
                .filter(e -> e.getKey().startsWith(normalizedPrefix)
                        && !e.getKey().equals(normalizedPrefix))
                .sorted(Map.Entry.comparingByKey())
                .map(e -> new KVEntry(e.getKey(), e.getValue(), versions.getOrDefault(e.getKey(), 1L), 1))
                .toList();
    }

    @Override
    public List<KVEntry> listRecursive(String prefix) throws KVStoreException {
        return list(prefix);
    }

    @Override
    public long put(String key, byte[] value) throws KVStoreException {
        if (failOnWrite.get()) {
            throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR, "Simulated write failure");
        }
        store.put(key, value);
        long ver = version.incrementAndGet();
        versions.put(key, ver);
        fireWatches(key, value, KVWatchEvent.Type.PUT, ver);
        return ver;
    }

    @Override
    public long cas(String key, byte[] value, long expectedVersion) throws KVStoreException {
        if (failOnWrite.get()) {
            throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR, "Simulated write failure");
        }
        synchronized (this) {
            if (expectedVersion == 0) {
                // Create-if-absent: key must not exist
                if (store.containsKey(key)) {
                    throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                            "Key already exists (CAS with expectedVersion=0): " + key);
                }
            } else {
                // Version must match
                Long currentVersion = versions.get(key);
                if (currentVersion == null || currentVersion != expectedVersion) {
                    throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                            "Version mismatch for key: " + key + ", expectedVersion=" + expectedVersion
                                    + ", currentVersion=" + currentVersion);
                }
            }
            store.put(key, value);
            long ver = version.incrementAndGet();
            versions.put(key, ver);
            fireWatches(key, value, KVWatchEvent.Type.PUT, ver);
            return ver;
        }
    }

    @Override
    public boolean delete(String key) throws KVStoreException {
        byte[] removed = store.remove(key);
        if (removed != null) {
            versions.remove(key);
            fireWatches(key, removed, KVWatchEvent.Type.DELETE, version.get());
            return true;
        }
        return false;
    }

    @Override
    public void deleteTree(String key) throws KVStoreException {
        store.keySet().removeIf(k -> k.startsWith(key));
        versions.keySet().removeIf(k -> k.startsWith(key));
    }

    @Override
    public Closeable watch(String keyOrPrefix, KVWatcher watcher) throws KVStoreException {
        WatchRegistration reg = new WatchRegistration(keyOrPrefix, watcher);
        watches.add(reg);
        return () -> watches.remove(reg);
    }

    @Override
    public Closeable acquireLock(String lockPath) throws KVStoreException {
        return () -> { };
    }

    @Override
    public LeaderElection leaderElection(String electionPath, String participantId) throws KVStoreException {
        return new LeaderElection() {
            private volatile boolean leader = true;
            private final CopyOnWriteArrayList<LeaderChangeListener> listeners = new CopyOnWriteArrayList<>();

            @Override
            public boolean isLeader() {
                return leader;
            }

            @Override
            public String currentLeaderId() {
                return participantId;
            }

            @Override
            public void addListener(LeaderChangeListener listener) {
                listeners.add(listener);
            }

            @Override
            public void close() {
                leader = false;
                for (LeaderChangeListener l : listeners) {
                    l.onLeaderChange(false);
                }
            }
        };
    }

    @Override
    public void close() {
        store.clear();
        versions.clear();
        watches.clear();
    }

    /**
     * Returns the current number of keys in the store.
     */
    public int size() {
        return store.size();
    }

    private void fireWatches(String key, byte[] value, KVWatchEvent.Type type, long ver) {
        KVEntry entry = new KVEntry(key, value, ver, 1);
        KVWatchEvent event = new KVWatchEvent(type, entry, null);
        for (WatchRegistration reg : watches) {
            if (key.startsWith(reg.prefix)) {
                try {
                    reg.watcher.onEvent(event);
                } catch (Exception e) {
                    // Swallow to match production watch behavior
                }
            }
        }
    }

    private record WatchRegistration(String prefix, KVWatcher watcher) { }
}
