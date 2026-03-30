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
package com.shieldblaze.expressgateway.servicediscovery.server;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.concurrent.CountDownLatch;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class RegistrationStoreTest {

    private RegistrationStore store;

    @BeforeEach
    void setup() {
        store = new RegistrationStore();
    }

    @Test
    void registerAndRetrieve() {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        RegistrationEntry entry = store.register(node, 60);

        assertNotNull(entry);
        assertTrue(entry.healthy());
        assertEquals(1, store.size());

        var retrieved = store.get("svc-1");
        assertTrue(retrieved.isPresent());
        assertEquals("svc-1", retrieved.get().node().id());
    }

    @Test
    void deregister() {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        store.register(node, 60);
        assertEquals(1, store.size());

        RegistrationEntry removed = store.deregister("svc-1");
        assertNotNull(removed);
        assertEquals(0, store.size());
    }

    @Test
    void deregisterNonexistent() {
        assertNull(store.deregister("nonexistent"));
    }

    @Test
    void heartbeatUpdatesTimestamp() throws InterruptedException {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        store.register(node, 60);

        var before = store.get("svc-1").get().lastHeartbeat();
        Thread.sleep(10);
        assertTrue(store.heartbeat("svc-1"));
        var after = store.get("svc-1").get().lastHeartbeat();

        assertTrue(after.isAfter(before));
    }

    @Test
    void heartbeatUnknownNode() {
        assertFalse(store.heartbeat("unknown"));
    }

    @Test
    void markUnhealthy() {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        store.register(node, 60);
        assertTrue(store.get("svc-1").get().healthy());

        store.markUnhealthy("svc-1");
        assertFalse(store.get("svc-1").get().healthy());
    }

    @Test
    void getHealthyFiltersUnhealthy() {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 60);
        store.register(new Node("svc-2", "10.0.0.2", 8081, false), 60);
        store.markUnhealthy("svc-2");

        assertEquals(1, store.getHealthy().size());
        assertEquals("svc-1", store.getHealthy().getFirst().node().id());
    }

    @Test
    void ttlExpiration() throws InterruptedException {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 1); // 1 second TTL
        assertFalse(store.get("svc-1").get().isExpired());

        Thread.sleep(1200);
        assertTrue(store.get("svc-1").get().isExpired());
    }

    @Test
    void evictExpired() throws InterruptedException {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 1);
        store.register(new Node("svc-2", "10.0.0.2", 8081, false), 0); // no TTL

        Thread.sleep(1200);

        int evicted = store.evictExpired();
        assertEquals(1, evicted);
        assertEquals(1, store.size()); // svc-2 should remain
        assertTrue(store.get("svc-2").isPresent());
    }

    @Test
    void reRegistrationUpdatesHeartbeat() throws InterruptedException {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 60);
        var firstHeartbeat = store.get("svc-1").get().lastHeartbeat();

        Thread.sleep(10);
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 60);
        var secondHeartbeat = store.get("svc-1").get().lastHeartbeat();

        assertTrue(secondHeartbeat.isAfter(firstHeartbeat));
        assertEquals(1, store.size()); // no duplicate entry
    }

    @Test
    void concurrentRegistrations() throws InterruptedException {
        int threadCount = 10;
        CountDownLatch latch = new CountDownLatch(threadCount);

        for (int i = 0; i < threadCount; i++) {
            int idx = i;
            Thread.ofVirtual().start(() -> {
                try {
                    store.register(new Node("svc-" + idx, "10.0.0." + idx, 8080 + idx, false), 60);
                } finally {
                    latch.countDown();
                }
            });
        }

        latch.await();
        assertEquals(threadCount, store.size());
    }

    @Test
    void clear() {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 60);
        store.register(new Node("svc-2", "10.0.0.2", 8081, false), 60);
        store.clear();
        assertEquals(0, store.size());
    }
}
