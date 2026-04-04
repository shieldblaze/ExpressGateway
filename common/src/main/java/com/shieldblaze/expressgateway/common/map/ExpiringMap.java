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
package com.shieldblaze.expressgateway.common.map;

import lombok.AccessLevel;
import lombok.Getter;
import lombok.experimental.Accessors;

import java.time.Duration;
import java.util.Collection;
import java.util.HashMap;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.BiConsumer;
import java.util.function.Function;

/**
 * Base Expiring Map Implementation.
 *
 * @param <K> Key
 * @param <V> Value
 */
public abstract class ExpiringMap<K, V> implements Map<K, V> {

    private final Map<K, V> storageMap;
    private final Map<Object, Long> timestampsMap = new ConcurrentHashMap<>();
    private final long ttlNanos;
    private final boolean autoRenew;

    @Getter(AccessLevel.PROTECTED)
    @Accessors(fluent = true)
    private final EntryRemovedListener<V> entryRemovedListener;

    /**
     * Create a new {@link ExpiringMap} Instance and use {@link HashMap} as
     * default {@code storageMap} and set {@code autoRenew} to {@code true}.
     *
     * @param ttlDuration TTL (Time-to-live) duration of entries
     */
    protected ExpiringMap(Duration ttlDuration) {
        this(new HashMap<>(), ttlDuration, true);
    }

    /**
     * Create a new {@link ExpiringMap} Instance.
     *
     * @param storageMap  {@link Map} Implementation to use for storing entries
     * @param ttlDuration TTL (Time-to-live) Duration of Entries
     * @param autoRenew   Set to {@code true} if entries will be auto-renewed on {@link #get(Object)} call
     *                    else set to {@code false}
     */
    protected ExpiringMap(Map<K, V> storageMap, Duration ttlDuration, boolean autoRenew) {
        this(storageMap, ttlDuration, autoRenew, new IgnoreEntryRemovedListener<>());
    }

    /**
     * Create a new {@link ExpiringMap} Instance.
     *
     * @param storageMap           {@link Map} Implementation to use for storing entries
     * @param ttlDuration          TTL (Time-to-live) Duration of Entries
     * @param autoRenew            Set to {@code true} if entries will be auto-renewed on {@link #get(Object)} call
     *                             else set to {@code false}
     * @param entryRemovedListener {@link EntryRemovedListener} Instance
     */
    protected ExpiringMap(Map<K, V> storageMap, Duration ttlDuration, boolean autoRenew, EntryRemovedListener<V> entryRemovedListener) {
        this.storageMap = Objects.requireNonNull(storageMap, "StorageMap");
        ttlNanos = ttlDuration.toNanos();
        this.autoRenew = autoRenew;

        if (!this.storageMap.isEmpty()) {
            throw new IllegalArgumentException("StorageMap Size must be Zero (0).");
        }

        this.entryRemovedListener = Objects.requireNonNull(entryRemovedListener, "EntryRemovedListener");
    }

    @Override
    public int size() {
        return storageMap.size();
    }

    @Override
    public boolean isEmpty() {
        return storageMap.isEmpty();
    }

    @Override
    public boolean containsKey(Object key) {
        return storageMap.containsKey(key);
    }

    @Override
    public boolean containsValue(Object value) {
        return storageMap.containsValue(value);
    }

    @Override
    public V get(Object key) {
        V v = storageMap.get(key);
        if (autoRenew) {
            timestampsMap.put(key, System.nanoTime());
        }
        return v;
    }

    @Override
    public V put(K key, V value) {
        timestampsMap.put(key, System.nanoTime());
        return storageMap.put(key, value);
    }

    @Override
    public V remove(Object key) {
        timestampsMap.remove(key);
        return storageMap.remove(key);
    }

    @Override
    public void putAll(Map<? extends K, ? extends V> m) {
        m.forEach((BiConsumer<K, V>) this::put);
    }

    @Override
    public void clear() {
        timestampsMap.clear();
        storageMap.clear();
    }

    @Override
    public Set<K> keySet() {
        return storageMap.keySet();
    }

    @Override
    public Collection<V> values() {
        return storageMap.values();
    }

    @Override
    public Set<Entry<K, V>> entrySet() {
        return storageMap.entrySet();
    }

    /**
     * Atomically compute a value if absent. When the underlying {@code storageMap}
     * is a {@link ConcurrentHashMap}, this inherits its per-bin locking — no global
     * synchronization is needed. The timestamp is recorded only when a new entry is
     * actually created, guaranteeing TTL accuracy.
     *
     * <p>If {@code autoRenew} is enabled and the key already exists, its timestamp
     * is refreshed — matching the behaviour of {@link #get(Object)}.
     *
     * @param key             the key
     * @param mappingFunction function to compute a value if absent
     * @return the existing or newly computed value
     */
    @Override
    public V computeIfAbsent(K key, Function<? super K, ? extends V> mappingFunction) {
        // Track whether the mapping function was invoked so we can distinguish
        // "existing hit" from "new creation" without a second map lookup.
        boolean[] created = {false};

        V value = storageMap.computeIfAbsent(key, k -> {
            V v = mappingFunction.apply(k);
            if (v != null) {
                timestampsMap.put(k, System.nanoTime());
                created[0] = true;
            }
            return v;
        });

        // Renew the timestamp on cache hit, consistent with get() auto-renew behaviour.
        if (!created[0] && autoRenew && value != null) {
            timestampsMap.put(key, System.nanoTime());
        }

        return value;
    }

    @Override
    public String toString() {
        return storageMap.toString();
    }

    protected boolean isExpired(Object key) {
        Long timestamp = timestampsMap.get(key);
        // Key may have been concurrently removed between keySet() iteration and this check
        if (timestamp == null) {
            return true;
        }
        return System.nanoTime() - timestamp > ttlNanos;
    }
}
