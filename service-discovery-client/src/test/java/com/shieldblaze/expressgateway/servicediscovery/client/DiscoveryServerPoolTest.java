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

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

class DiscoveryServerPoolTest {

    @Test
    void roundRobinSelection() {
        DiscoveryServerPool pool = new DiscoveryServerPool(
                List.of("http://server1", "http://server2", "http://server3"), 3);

        String first = pool.selectServer();
        String second = pool.selectServer();
        String third = pool.selectServer();

        // Round-robin should cycle through servers
        assertNotNull(first);
        assertNotNull(second);
        assertNotNull(third);
        assertEquals(3, pool.size());
    }

    @Test
    void skipsUnhealthyServers() {
        DiscoveryServerPool pool = new DiscoveryServerPool(
                List.of("http://server1", "http://server2"), 2);

        // Mark server1 as unhealthy (2 consecutive failures)
        pool.recordFailure("http://server1");
        pool.recordFailure("http://server1");

        assertEquals(1, pool.healthyCount());

        // Should only select server2 now
        for (int i = 0; i < 10; i++) {
            String selected = pool.selectServer();
            assertEquals("http://server2", selected);
        }
    }

    @Test
    void successResetsHealth() {
        DiscoveryServerPool pool = new DiscoveryServerPool(
                List.of("http://server1"), 2);

        pool.recordFailure("http://server1");
        pool.recordSuccess("http://server1");
        assertEquals(1, pool.healthyCount());
    }

    @Test
    void allUnhealthyReturnsLeastRecentlyFailed() throws InterruptedException {
        DiscoveryServerPool pool = new DiscoveryServerPool(
                List.of("http://server1", "http://server2"), 1);

        // Fail server1 first, then server2
        pool.recordFailure("http://server1");
        Thread.sleep(10);
        pool.recordFailure("http://server2");

        assertEquals(0, pool.healthyCount());

        // Should pick server1 (oldest failure)
        String selected = pool.selectServer();
        assertEquals("http://server1", selected);
    }

    @Test
    void resetAllRestoresHealth() {
        DiscoveryServerPool pool = new DiscoveryServerPool(
                List.of("http://server1", "http://server2"), 1);

        pool.recordFailure("http://server1");
        pool.recordFailure("http://server2");
        assertEquals(0, pool.healthyCount());

        pool.resetAll();
        assertEquals(2, pool.healthyCount());
    }

    @Test
    void emptyServerListThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> new DiscoveryServerPool(List.of(), 3));
    }

    @Test
    void invalidThresholdThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> new DiscoveryServerPool(List.of("http://server1"), 0));
    }

    @Test
    void serverUrisReturnsAll() {
        DiscoveryServerPool pool = new DiscoveryServerPool(
                List.of("http://a", "http://b"), 3);
        assertEquals(List.of("http://a", "http://b"), pool.serverUris());
    }
}
