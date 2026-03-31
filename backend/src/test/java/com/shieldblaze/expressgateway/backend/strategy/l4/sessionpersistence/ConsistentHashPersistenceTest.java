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
import com.shieldblaze.expressgateway.backend.strategy.l4.ConsistentHash;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

class ConsistentHashPersistenceTest {

    @Test
    void testSameClientAddressMapsToSameNode() throws Exception {
        ConsistentHashPersistence persistence = new ConsistentHashPersistence();
        ConsistentHash lb = new ConsistentHash(persistence);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(lb).build();

        for (int i = 1; i <= 5; i++) {
            NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("10.0.0." + i, 8080))
                    .build();
        }

        // Same client address should always map to the same node
        InetSocketAddress client = new InetSocketAddress("192.168.1.100", 12345);
        L4Request req = new L4Request(client);

        Node first = cluster.nextNode(req).node();
        assertNotNull(first);
        for (int i = 0; i < 50; i++) {
            Node next = cluster.nextNode(req).node();
            assertEquals(first, next, "Same client address should always map to same backend node");
        }

        cluster.close();
    }

    @Test
    void testEmptyRingReturnsNull() {
        ConsistentHashPersistence persistence = new ConsistentHashPersistence();
        L4Request req = new L4Request(new InetSocketAddress("192.168.1.100", 12345));
        assertNull(persistence.node(req), "Empty ring should return null");
    }

    @Test
    void testAddRouteIsNoOp() throws Exception {
        ConsistentHashPersistence persistence = new ConsistentHashPersistence();
        ConsistentHash lb = new ConsistentHash(persistence);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(lb).build();

        // addRoute should return the node unchanged (ring-based, no per-session state)
        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();

        Node result = persistence.addRoute(new InetSocketAddress("192.168.1.1", 1234), node);
        assertEquals(node, result, "addRoute should return the same node");

        cluster.close();
    }

    @Test
    void testClearEmptiesRing() throws Exception {
        ConsistentHashPersistence persistence = new ConsistentHashPersistence();
        ConsistentHash lb = new ConsistentHash(persistence);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(lb).build();

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();

        persistence.clear();

        // After clear, the ring should be empty
        L4Request req = new L4Request(new InetSocketAddress("192.168.1.100", 12345));
        assertNull(persistence.node(req), "After clear, ring should return null");

        cluster.close();
    }

    @Test
    void testNameReturnsExpected() {
        ConsistentHashPersistence persistence = new ConsistentHashPersistence();
        assertEquals("ConsistentHashPersistence", persistence.name());
    }

    @Test
    void testDeterministicMapping() throws Exception {
        ConsistentHashPersistence persistence = new ConsistentHashPersistence();
        ConsistentHash lb = new ConsistentHash(persistence);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(lb).build();

        for (int i = 1; i <= 10; i++) {
            NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("10.0.0." + i, 8080))
                    .build();
        }

        // Multiple lookups for the same client should always return the same node
        L4Request req = new L4Request(new InetSocketAddress("192.168.1.50", 9999));
        Node first = cluster.nextNode(req).node();
        for (int i = 0; i < 100; i++) {
            Node next = cluster.nextNode(req).node();
            assertEquals(first, next, "Deterministic: same request should always map to same node");
        }

        cluster.close();
    }
}
