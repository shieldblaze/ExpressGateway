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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import io.netty.channel.ChannelFuture;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class NodeTest {

    @Test
    void maxConnectionTest() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE)).build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(9110))
                .build();

        assertThrows(IllegalArgumentException.class, () -> node.maxConnections(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> node.maxConnections(-2));
        assertDoesNotThrow(() -> node.maxConnections(1));
        assertDoesNotThrow(() -> node.maxConnections(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> node.maxConnections(5000));

        // Add 5000 fake connections
        for (int i = 0; i < 5000; i++) {
            node.incActiveConnection0();
        }

        // Add 1 more connection and it should throw TooManyConnectionsException.
        assertThrows(TooManyConnectionsException.class, () -> node.addConnection(new DummyConnection(node)));

        // Close Cluster to prevent memory leaks.
        cluster.close();
    }

    /**
     * Verifies that activeConnection() returns the correct sum of both
     * counter mechanisms:
     *   1. The connection queue (managed via addConnection/removeConnection)
     *   2. The secondary atomic counter (managed via incActiveConnection0/decActiveConnection0)
     *
     * This ensures the dual-counter model does not silently under-count or
     * over-count, which would break LeastConnection load balancing.
     */
    @Test
    void activeConnectionCounterTest() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(9111))
                .build();

        // Initially, both counters are zero
        assertEquals(0, node.activeConnection(),
                "activeConnection must be 0 when no connections exist");
        assertEquals(0, node.activeConnection0(),
                "activeConnection0 counter must start at 0");

        // Increment secondary counter only
        node.incActiveConnection0();
        node.incActiveConnection0();
        node.incActiveConnection0();
        assertEquals(3, node.activeConnection0(), "activeConnection0 must be 3");
        assertEquals(3, node.activeConnection(),
                "activeConnection must return sum: 0 (queue) + 3 (counter)");

        // Decrement secondary counter
        node.decActiveConnection0();
        assertEquals(2, node.activeConnection0(), "activeConnection0 must be 2 after decrement");
        assertEquals(2, node.activeConnection(),
                "activeConnection must return sum: 0 (queue) + 2 (counter)");

        // Reset secondary counter
        node.resetActiveConnection0();
        assertEquals(0, node.activeConnection0(), "activeConnection0 must be 0 after reset");
        assertEquals(0, node.activeConnection(),
                "activeConnection must return 0 after reset");

        // Now add connections via the queue path as well
        node.maxConnections(100);
        DummyConnection conn1 = new DummyConnection(node);
        DummyConnection conn2 = new DummyConnection(node);
        node.addConnection(conn1);
        node.addConnection(conn2);
        assertEquals(2, node.activeConnection(),
                "activeConnection must count 2 queue connections");

        // Combine both counters
        node.incActiveConnection0();
        node.incActiveConnection0();
        node.incActiveConnection0();
        assertEquals(5, node.activeConnection(),
                "activeConnection must be sum: 2 (queue) + 3 (counter)");

        // Remove a queue connection
        node.removeConnection(conn1);
        assertEquals(4, node.activeConnection(),
                "activeConnection must be sum: 1 (queue) + 3 (counter)");

        // Verify connectionFull respects combined count
        node.maxConnections(4);
        assertTrue(node.connectionFull(),
                "connectionFull must consider combined count (4 >= 4)");

        // Close Cluster to prevent memory leaks.
        cluster.close();
    }

    @Test
    void equalsAndHashCodeContract() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node nodeA = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", 8080))
                .build();

        Node nodeB = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", 8080))
                .build();

        Node nodeC = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", 9090))
                .build();

        // Reflexive
        assertEquals(nodeA, nodeA);

        // Symmetric: same address means equal in both directions
        assertEquals(nodeA, nodeB);
        assertEquals(nodeB, nodeA);

        // Different UUIDs must not break equality
        assertNotEquals(nodeA.id(), nodeB.id(), "Precondition: nodes have distinct UUIDs");
        assertEquals(nodeA, nodeB, "Nodes with same socketAddress must be equal regardless of UUID");

        // hashCode contract: equal objects must have equal hash codes
        assertEquals(nodeA.hashCode(), nodeB.hashCode());

        // Nodes with different addresses are not equal
        assertNotEquals(nodeA, nodeC);

        // Null and foreign types
        assertNotEquals(nodeA, null);
        assertNotEquals(nodeA, "not a node");

        // HashMap correctness
        Map<Node, String> map = new HashMap<>();
        map.put(nodeA, "backend-1");
        assertEquals("backend-1", map.get(nodeB));
        assertTrue(map.containsKey(nodeB));

        // HashSet deduplication
        Set<Node> set = new HashSet<>();
        set.add(nodeA);
        set.add(nodeB);
        assertEquals(1, set.size());

        cluster.close();
    }

    private static final class DummyConnection extends Connection {

        private DummyConnection(Node node) {
            super(node);
        }

        @Override
        protected void processBacklog(ChannelFuture channelFuture) {
            // Does nothing
        }
    }
}
