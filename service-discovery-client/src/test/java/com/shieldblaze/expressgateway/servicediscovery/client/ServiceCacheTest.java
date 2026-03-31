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

import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Optional;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ServiceCacheTest {

    @Test
    void putAndGet() {
        ServiceCache cache = new ServiceCache(60_000);
        ServiceEntry entry = ServiceEntry.of("svc-1", "10.0.0.1", 8080, false);
        cache.put("svc-1", entry);

        Optional<ServiceCache.CacheResult> result = cache.get("svc-1");
        assertTrue(result.isPresent());
        assertTrue(result.get().fresh());
        assertEquals("svc-1", result.get().entry().id());
    }

    @Test
    void getMissingReturnsEmpty() {
        ServiceCache cache = new ServiceCache(60_000);
        assertTrue(cache.get("nonexistent").isEmpty());
    }

    @Test
    void ttlExpiration() throws InterruptedException {
        ServiceCache cache = new ServiceCache(50);
        ServiceEntry entry = ServiceEntry.of("svc-1", "10.0.0.1", 8080, false);
        cache.put("svc-1", entry);

        // Should be fresh immediately
        assertTrue(cache.get("svc-1").get().fresh());

        // Wait for TTL to expire
        Thread.sleep(100);

        // Should still be present but stale
        Optional<ServiceCache.CacheResult> result = cache.get("svc-1");
        assertTrue(result.isPresent());
        assertFalse(result.get().fresh());
    }

    @Test
    void putAll() {
        ServiceCache cache = new ServiceCache(60_000);
        List<ServiceEntry> entries = List.of(
                ServiceEntry.of("svc-1", "10.0.0.1", 8080, false),
                ServiceEntry.of("svc-2", "10.0.0.2", 8081, true));
        cache.putAll(entries);
        assertEquals(2, cache.size());
    }

    @Test
    void getAll() {
        ServiceCache cache = new ServiceCache(60_000);
        cache.put("svc-1", ServiceEntry.of("svc-1", "10.0.0.1", 8080, false));
        cache.put("svc-2", ServiceEntry.of("svc-2", "10.0.0.2", 8081, true));
        assertEquals(2, cache.getAll().size());
    }

    @Test
    void getFreshFiltersExpired() throws InterruptedException {
        ServiceCache cache = new ServiceCache(50);
        cache.put("svc-1", ServiceEntry.of("svc-1", "10.0.0.1", 8080, false));
        assertEquals(1, cache.getFresh().size());

        Thread.sleep(100);
        assertEquals(0, cache.getFresh().size());
        assertEquals(1, cache.getAll().size()); // stale entries still in cache
    }

    @Test
    void remove() {
        ServiceCache cache = new ServiceCache(60_000);
        cache.put("svc-1", ServiceEntry.of("svc-1", "10.0.0.1", 8080, false));
        assertEquals(1, cache.size());
        cache.remove("svc-1");
        assertEquals(0, cache.size());
    }

    @Test
    void clear() {
        ServiceCache cache = new ServiceCache(60_000);
        cache.put("svc-1", ServiceEntry.of("svc-1", "10.0.0.1", 8080, false));
        cache.put("svc-2", ServiceEntry.of("svc-2", "10.0.0.2", 8081, true));
        cache.clear();
        assertEquals(0, cache.size());
    }

    @Test
    void invalidTtlThrows() {
        assertThrows(IllegalArgumentException.class, () -> new ServiceCache(0));
        assertThrows(IllegalArgumentException.class, () -> new ServiceCache(-100));
    }

    @Test
    void overwriteExistingEntry() {
        ServiceCache cache = new ServiceCache(60_000);
        cache.put("svc-1", ServiceEntry.of("svc-1", "10.0.0.1", 8080, false));
        cache.put("svc-1", ServiceEntry.of("svc-1", "10.0.0.2", 9090, true));

        Optional<ServiceCache.CacheResult> result = cache.get("svc-1");
        assertTrue(result.isPresent());
        assertEquals("10.0.0.2", result.get().entry().ipAddress());
        assertEquals(9090, result.get().entry().port());
    }
}
