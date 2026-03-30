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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTPLeastResponseTimeTest {

    /**
     * When all nodes are warm (above the cold start threshold), the load balancer
     * must select the node with the lowest EWMA response time.
     */
    @Test
    void testSelectsLowestResponseTime() throws Exception {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);
        HTTPLeastResponseTime lb = new HTTPLeastResponseTime(NOOPSessionPersistence.INSTANCE, tracker);
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        Node fast = fastBuild(cluster, "10.0.0.1");
        Node medium = fastBuild(cluster, "10.0.0.2");
        Node slow = fastBuild(cluster, "10.0.0.3");

        // Warm up all nodes past the cold start threshold (10 samples).
        for (int i = 0; i < ResponseTimeTracker.COLD_START_THRESHOLD + 5; i++) {
            tracker.record(fast, 10);    // 10ms -- fast
            tracker.record(medium, 50);  // 50ms -- medium
            tracker.record(slow, 200);   // 200ms -- slow
        }

        assertTrue(tracker.isWarm(fast), "fast node should be warm");
        assertTrue(tracker.isWarm(medium), "medium node should be warm");
        assertTrue(tracker.isWarm(slow), "slow node should be warm");

        HTTPBalanceRequest request = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        // When all nodes are warm, the load balancer should consistently pick the fastest node.
        Map<InetSocketAddress, Integer> selectionCounts = new HashMap<>();
        int totalRequests = 100;
        for (int i = 0; i < totalRequests; i++) {
            Node selected = cluster.nextNode(request).node();
            selectionCounts.merge(selected.socketAddress(), 1, Integer::sum);
        }

        // The fast node (10ms EWMA) must be selected every time since it has the lowest EWMA.
        assertEquals(totalRequests, selectionCounts.getOrDefault(fast.socketAddress(), 0),
                "When all warm, the node with the lowest EWMA must always be selected. "
                        + "Selections: " + selectionCounts);
    }

    /**
     * When all nodes are cold (below the sample threshold), the load balancer must
     * distribute requests via round-robin to gather initial samples evenly.
     */
    @Test
    void testColdStartRoundRobin() throws Exception {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);
        HTTPLeastResponseTime lb = new HTTPLeastResponseTime(NOOPSessionPersistence.INSTANCE, tracker);
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        Node node1 = fastBuild(cluster, "10.0.0.1");
        Node node2 = fastBuild(cluster, "10.0.0.2");
        Node node3 = fastBuild(cluster, "10.0.0.3");

        // Do not record any samples -- all nodes are cold.
        assertEquals(0, tracker.getSampleCount(node1));
        assertEquals(0, tracker.getSampleCount(node2));
        assertEquals(0, tracker.getSampleCount(node3));

        HTTPBalanceRequest request = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        // Issue enough requests to observe round-robin distribution across all 3 nodes.
        Map<InetSocketAddress, Integer> selectionCounts = new HashMap<>();
        int totalRequests = 30;
        for (int i = 0; i < totalRequests; i++) {
            Node selected = cluster.nextNode(request).node();
            selectionCounts.merge(selected.socketAddress(), 1, Integer::sum);
        }

        // Each node must receive at least some requests (round-robin distributes evenly).
        assertEquals(3, selectionCounts.size(),
                "All 3 cold nodes should receive traffic. Selections: " + selectionCounts);

        for (Map.Entry<InetSocketAddress, Integer> entry : selectionCounts.entrySet()) {
            assertTrue(entry.getValue() >= totalRequests / 3 - 2,
                    "Node " + entry.getKey() + " received " + entry.getValue()
                            + " requests, expected roughly " + (totalRequests / 3));
        }
    }

    /**
     * When some nodes are warm and others are cold, the load balancer must prefer
     * cold nodes to gather samples before relying on EWMA values.
     */
    @Test
    void testColdNodesPreferred() throws Exception {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);
        HTTPLeastResponseTime lb = new HTTPLeastResponseTime(NOOPSessionPersistence.INSTANCE, tracker);
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        Node warmNode = fastBuild(cluster, "10.0.0.1");
        Node coldNode1 = fastBuild(cluster, "10.0.0.2");
        Node coldNode2 = fastBuild(cluster, "10.0.0.3");

        // Warm up only the first node past the threshold.
        for (int i = 0; i < ResponseTimeTracker.COLD_START_THRESHOLD + 5; i++) {
            tracker.record(warmNode, 10); // very fast, but it should still not be preferred
        }

        assertTrue(tracker.isWarm(warmNode), "warmNode should be warm");
        assertTrue(!tracker.isWarm(coldNode1), "coldNode1 should be cold");
        assertTrue(!tracker.isWarm(coldNode2), "coldNode2 should be cold");

        HTTPBalanceRequest request = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        // All selections should go to cold nodes, never the warm node.
        Set<InetSocketAddress> selectedAddresses = new HashSet<>();
        int totalRequests = 20;
        for (int i = 0; i < totalRequests; i++) {
            Node selected = cluster.nextNode(request).node();
            selectedAddresses.add(selected.socketAddress());
        }

        assertTrue(!selectedAddresses.contains(warmNode.socketAddress()),
                "When cold nodes exist, the warm node should NOT be selected. "
                        + "Selected addresses: " + selectedAddresses);
        assertTrue(selectedAddresses.contains(coldNode1.socketAddress())
                        || selectedAddresses.contains(coldNode2.socketAddress()),
                "At least one cold node must be selected");
    }

    /**
     * Recording response time samples must correctly update the EWMA value.
     * With alpha=1.0, each new sample completely replaces the EWMA, making
     * the math deterministic and easy to verify.
     */
    @Test
    void testRecordResponseTime() throws Exception {
        // alpha=1.0 means: ewma = 1.0 * sample + 0.0 * old = sample
        ResponseTimeTracker tracker = new ResponseTimeTracker(1.0);
        HTTPLeastResponseTime lb = new HTTPLeastResponseTime(NOOPSessionPersistence.INSTANCE, tracker);
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        Node node = fastBuild(cluster, "10.0.0.1");

        // First sample initializes the EWMA.
        lb.recordResponseTime(node, 100);
        assertEquals(100.0, tracker.getEwma(node), 0.001,
                "First sample should initialize EWMA to 100");
        assertEquals(1, tracker.getSampleCount(node));

        // With alpha=1.0, each subsequent sample fully replaces the EWMA.
        lb.recordResponseTime(node, 200);
        assertEquals(200.0, tracker.getEwma(node), 0.001,
                "With alpha=1.0, EWMA should equal the latest sample");
        assertEquals(2, tracker.getSampleCount(node));

        lb.recordResponseTime(node, 50);
        assertEquals(50.0, tracker.getEwma(node), 0.001);
        assertEquals(3, tracker.getSampleCount(node));

        // Now verify with alpha=0.5 for proper EWMA behavior.
        ResponseTimeTracker tracker2 = new ResponseTimeTracker(0.5);
        Node node2 = fastBuild(cluster, "10.0.0.2");

        tracker2.record(node2, 100); // First: ewma = 100
        assertEquals(100.0, tracker2.getEwma(node2), 0.001);

        tracker2.record(node2, 200); // ewma = 0.5*200 + 0.5*100 = 150
        assertEquals(150.0, tracker2.getEwma(node2), 0.001);

        tracker2.record(node2, 200); // ewma = 0.5*200 + 0.5*150 = 175
        assertEquals(175.0, tracker2.getEwma(node2), 0.001);
    }

    /**
     * When a node is removed from the cluster, the ResponseTimeTracker must clear
     * its EWMA state for that node. This prevents stale data from affecting
     * selection if a node with the same address is re-added later.
     */
    @Test
    void testNodeRemovalCleansTracker() throws Exception {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);
        HTTPLeastResponseTime lb = new HTTPLeastResponseTime(NOOPSessionPersistence.INSTANCE, tracker);
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        Node node1 = fastBuild(cluster, "10.0.0.1");
        Node node2 = fastBuild(cluster, "10.0.0.2");

        // Record enough samples to make the node warm.
        for (int i = 0; i < ResponseTimeTracker.COLD_START_THRESHOLD + 5; i++) {
            tracker.record(node1, 100);
            tracker.record(node2, 200);
        }

        assertTrue(tracker.isWarm(node1));
        assertTrue(tracker.isWarm(node2));
        assertTrue(tracker.getEwma(node1) > 0);

        // Close node1 (triggers NodeRemovedTask -> accept() -> tracker.remove(node)).
        node1.close();

        // After removal, the tracker should have no state for node1.
        assertEquals(0, tracker.getSampleCount(node1),
                "After removal, sample count should be 0");
        assertEquals(0.0, tracker.getEwma(node1), 0.001,
                "After removal, EWMA should be 0.0");
        assertTrue(!tracker.isWarm(node1),
                "After removal, node should not be warm");

        // node2 should be unaffected.
        assertTrue(tracker.isWarm(node2));
        assertTrue(tracker.getEwma(node2) > 0);
    }

    /**
     * When there are no online nodes in the cluster, response() must throw
     * NoNodeAvailableException.
     */
    @Test
    void testEmptyClusterThrows() {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPLeastResponseTime(NOOPSessionPersistence.INSTANCE))
                .build();

        HTTPBalanceRequest request = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        assertThrows(NoNodeAvailableException.class, () -> cluster.nextNode(request));
    }

    private static Node fastBuild(Cluster cluster, String host) throws Exception {
        return NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
