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
package com.shieldblaze.expressgateway.servicediscovery.client;

import java.util.List;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentMap;

/**
 * Thread-safe local cache for service discovery entries with TTL-based expiration.
 * When the discovery server is unreachable, stale cache entries are still returned
 * as a last-resort fallback to maintain availability.
 *
 * <p>The cache uses a simple time-based eviction strategy. Entries are not proactively
 * removed; they are checked for freshness on read. This avoids background threads and
 * keeps the cache lightweight.</p>
 *
 * <p>Uses monotonic {@link System#nanoTime()} for TTL checks to avoid wall-clock
 * drift caused by NTP adjustments or system clock changes.</p>
 */
public final class ServiceCache {

    private final long ttlNanos;
    private final long ttlMillis;
    private final ConcurrentMap<String, CachedEntry> cache = new ConcurrentHashMap<>();

    /**
     * @param ttlMillis time-to-live for cache entries in milliseconds
     */
    public ServiceCache(long ttlMillis) {
        if (ttlMillis <= 0) {
            throw new IllegalArgumentException("ttlMillis must be positive, got: " + ttlMillis);
        }
        this.ttlMillis = ttlMillis;
        this.ttlNanos = ttlMillis * 1_000_000L;
    }

    /**
     * Store a service entry in the cache, keyed by service ID.
     */
    public void put(String serviceId, ServiceEntry entry) {
        cache.put(serviceId, new CachedEntry(entry, System.nanoTime()));
    }

    /**
     * Store multiple service entries.
     */
    public void putAll(List<ServiceEntry> entries) {
        long now = System.nanoTime();
        for (ServiceEntry entry : entries) {
            cache.put(entry.id(), new CachedEntry(entry, now));
        }
    }

    /**
     * Get a service entry by ID. Returns empty if not cached.
     * Returns the entry regardless of freshness (caller decides via {@link CacheResult}).
     */
    public Optional<CacheResult> get(String serviceId) {
        CachedEntry cached = cache.get(serviceId);
        if (cached == null) {
            return Optional.empty();
        }
        boolean fresh = isFresh(cached);
        return Optional.of(new CacheResult(cached.entry, fresh));
    }

    /**
     * Get all cached entries (both fresh and stale).
     */
    public List<ServiceEntry> getAll() {
        return cache.values().stream()
                .map(CachedEntry::entry)
                .toList();
    }

    /**
     * Get only fresh (non-expired) entries.
     */
    public List<ServiceEntry> getFresh() {
        return cache.values().stream()
                .filter(this::isFresh)
                .map(CachedEntry::entry)
                .toList();
    }

    /**
     * Remove a specific entry from the cache.
     */
    public void remove(String serviceId) {
        cache.remove(serviceId);
    }

    /**
     * Clear the entire cache.
     */
    public void clear() {
        cache.clear();
    }

    /**
     * Return the number of entries in the cache (including stale).
     */
    public int size() {
        return cache.size();
    }

    /**
     * Return the configured TTL in milliseconds.
     */
    public long ttlMillis() {
        return ttlMillis;
    }

    private boolean isFresh(CachedEntry entry) {
        return System.nanoTime() - entry.cachedAtNanos < ttlNanos;
    }

    /**
     * Result of a cache lookup, indicating whether the entry is fresh or stale.
     */
    public record CacheResult(ServiceEntry entry, boolean fresh) {
    }

    private record CachedEntry(ServiceEntry entry, long cachedAtNanos) {
    }
}
