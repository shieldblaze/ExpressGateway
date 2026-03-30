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
package com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link FourTupleHash} session persistence.
 *
 * <p>Validates route addition and retrieval, eviction when capacity
 * is exceeded, and that different 4-tuples get different routes.</p>
 */
class FourTupleHashTest {

    /**
     * Test adding a route and retrieving it by the same 4-tuple.
     */
    @Test
    void addRouteAndRetrieve() throws Exception {
        FourTupleHash fourTupleHash = new FourTupleHash();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node1 = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();

        InetSocketAddress clientAddr = new InetSocketAddress("192.168.1.100", 54321);

        // Add a route
        Node added = fourTupleHash.addRoute(clientAddr, node1);
        assertSame(node1, added, "addRoute should return the added node");

        // Retrieve the route by the same address
        L4Request request = new L4Request(clientAddr);
        Node retrieved = fourTupleHash.node(request);
        assertNotNull(retrieved, "Should retrieve the route for the same 4-tuple");
        assertSame(node1, retrieved, "Retrieved node should be the same as the added one");
    }

    /**
     * Test that different 4-tuples get different routes.
     */
    @Test
    void differentTuplesGetDifferentRoutes() throws Exception {
        FourTupleHash fourTupleHash = new FourTupleHash();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node1 = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();

        Node node2 = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.2", 8080))
                .build();

        InetSocketAddress client1 = new InetSocketAddress("192.168.1.100", 54321);
        InetSocketAddress client2 = new InetSocketAddress("192.168.1.200", 54322);

        fourTupleHash.addRoute(client1, node1);
        fourTupleHash.addRoute(client2, node2);

        Node retrieved1 = fourTupleHash.node(new L4Request(client1));
        Node retrieved2 = fourTupleHash.node(new L4Request(client2));

        assertSame(node1, retrieved1, "Client1 should map to node1");
        assertSame(node2, retrieved2, "Client2 should map to node2");
        assertNotEquals(retrieved1, retrieved2, "Different 4-tuples should map to different nodes");
    }

    /**
     * Test that looking up a non-existent 4-tuple returns null.
     */
    @Test
    void lookupNonExistentTupleReturnsNull() {
        FourTupleHash fourTupleHash = new FourTupleHash();

        InetSocketAddress unknownClient = new InetSocketAddress("10.10.10.10", 12345);
        Node result = fourTupleHash.node(new L4Request(unknownClient));
        assertNull(result, "Non-existent 4-tuple should return null");
    }

    /**
     * Test eviction when capacity is exceeded.
     * Uses a small maxEntries to trigger eviction.
     */
    @Test
    void evictionWhenCapacityExceeded() throws Exception {
        int maxEntries = 10;
        FourTupleHash fourTupleHash = new FourTupleHash(maxEntries);

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();

        // Fill the map to capacity
        for (int i = 0; i < maxEntries; i++) {
            fourTupleHash.addRoute(new InetSocketAddress("192.168.1." + (i + 1), 10000 + i), node);
        }
        assertEquals(maxEntries, fourTupleHash.size(), "Map should be at capacity");

        // Add one more entry -- this should trigger eviction of ~10% (1 entry min)
        fourTupleHash.addRoute(new InetSocketAddress("192.168.2.1", 20000), node);

        // After eviction, size should be less than maxEntries + 1 because some were evicted
        assertTrue(fourTupleHash.size() <= maxEntries,
                "Map size should not exceed maxEntries after eviction, got: " + fourTupleHash.size());

        // The newly added entry should still be present
        Node recent = fourTupleHash.node(new L4Request(new InetSocketAddress("192.168.2.1", 20000)));
        assertNotNull(recent, "Most recently added entry should survive eviction");
    }

    /**
     * Test removeRoute removes a specific mapping.
     */
    @Test
    void removeRoute() throws Exception {
        FourTupleHash fourTupleHash = new FourTupleHash();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();

        InetSocketAddress clientAddr = new InetSocketAddress("192.168.1.1", 5000);
        fourTupleHash.addRoute(clientAddr, node);

        boolean removed = fourTupleHash.removeRoute(clientAddr, node);
        assertTrue(removed, "removeRoute should return true for existing mapping");

        Node result = fourTupleHash.node(new L4Request(clientAddr));
        assertNull(result, "Removed route should not be retrievable");
    }

    /**
     * Test clear removes all entries.
     */
    @Test
    void clearRemovesAllEntries() throws Exception {
        FourTupleHash fourTupleHash = new FourTupleHash();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();

        fourTupleHash.addRoute(new InetSocketAddress("192.168.1.1", 5000), node);
        fourTupleHash.addRoute(new InetSocketAddress("192.168.1.2", 5001), node);

        assertEquals(2, fourTupleHash.size());
        fourTupleHash.clear();
        assertEquals(0, fourTupleHash.size(), "Map should be empty after clear");
    }
}
