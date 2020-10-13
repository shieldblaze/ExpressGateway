/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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

import java.time.Duration;
import java.util.Collection;
import java.util.HashMap;
import java.util.Map;
import java.util.Set;

/**
 * {@link ManualExpiringMap} removes expired entries whenever following methods are called:
 * <ul>
 *     <li> {@link #containsKey(Object)} </li>
 *     <li> {@link #get(Object)} </li>
 *     <li> {@link #putAll(Map)} </li>
 *     <li> {@link #keySet()} </li>
 *     <li> {@link #values()} </li>
 *     <li> {@link #toString()} </li>
 * </ul>
 *
 * @param <K> Key
 * @param <V> Value
 */
public final class ManualExpiringMap<K, V> extends ExpiringMap<K, V> {

    /**
     * Create a new {@link ManualExpiringMap} Instance and use {@link HashMap} as
     * default {@code storageMap} and set {@code autoRenew} to {@code true}.
     *
     * @param ttlDuration TTL (Time-to-live) duration of entries
     */
    public ManualExpiringMap(Duration ttlDuration) {
        super(ttlDuration);
    }

    /**
     * Creates a new {@link ManualExpiringMap} Instance.
     *
     * @param storageMap  {@link Map} Implementation to use for storing entries
     * @param ttlDuration TTL (Time-to-live) Duration of Entries
     * @param autoRenew   Set to {@code true} if entries will be auto-renewed on {@link #get(Object)} call
     *                    else set to {@code false}
     */
    public ManualExpiringMap(Map<K, V> storageMap, Duration ttlDuration, boolean autoRenew) {
        super(storageMap, ttlDuration, autoRenew);
    }

    @Override
    public int size() {
        return super.size();
    }

    @Override
    public boolean isEmpty() {
        return super.isEmpty();
    }

    @Override
    public boolean containsKey(Object key) {
        if (isExpired(key)) {
            remove(key);
            return false;
        }
        return super.containsKey(key);
    }

    @Override
    public boolean containsValue(Object value) {
        return super.containsValue(value);
    }

    @Override
    public V get(Object key) {
        if (isExpired(key)) {
            remove(key);
            return null;
        }
        return super.get(key);
    }

    @Override
    public V put(K key, V value) {
        return super.put(key, value);
    }

    @Override
    public V remove(Object key) {
        return super.remove(key);
    }

    @Override
    public void putAll(Map<? extends K, ? extends V> m) {
        super.putAll(m);
        cleanUp();
    }

    @Override
    public void clear() {
        super.clear();
    }

    @Override
    public Set<K> keySet() {
        cleanUp();
        return super.keySet();
    }

    @Override
    public Collection<V> values() {
        cleanUp();
        return super.values();
    }

    @Override
    public Set<Entry<K, V>> entrySet() {
        return super.entrySet();
    }

    @Override
    public String toString() {
        cleanUp();
        return super.toString();
    }

    private void cleanUp() {
        forEach((k, v) -> {
            if (isExpired(k)) {
                remove(k);
            }
        });
    }
}
