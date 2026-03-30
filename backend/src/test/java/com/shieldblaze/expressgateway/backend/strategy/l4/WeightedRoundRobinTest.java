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
package com.shieldblaze.expressgateway.backend.strategy.l4;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class WeightedRoundRobinTest {

    /**
     * Three nodes with equal weight (default = 1) should each receive
     * exactly 1/3 of the total selections over a full cycle.
     */
    @Test
    void testEqualWeightDistribution() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        fastBuild(cluster, "10.0.0.1");
        fastBuild(cluster, "10.0.0.2");
        fastBuild(cluster, "10.0.0.3");

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        Map<String, Integer> counts = new HashMap<>();
        int totalSelections = 3000;

        for (int i = 0; i < totalSelections; i++) {
            Node node = cluster.nextNode(request).node();
            counts.merge(node.socketAddress().getHostString(), 1, Integer::sum);
        }

        // With equal weights, each node gets exactly 1/3 of selections.
        // The smooth WRR algorithm is deterministic so distribution is exact.
        assertEquals(1000, counts.get("10.0.0.1"));
        assertEquals(1000, counts.get("10.0.0.2"));
        assertEquals(1000, counts.get("10.0.0.3"));

        cluster.close();
    }

    /**
     * Verifies the NGINX-style smooth interleaved distribution.
     * With weights {A=5, B=1, C=1}, the algorithm produces a sequence where
     * selections are spread out (e.g. A,A,B,A,C,A,A) rather than clustered
     * (e.g. A,A,A,A,A,B,C).
     *
     * The canonical NGINX smooth weighted round-robin sequence for {5,1,1}:
     *   Round 1: cw={5,1,1}  -> pick A(5), cw={-2,1,1}
     *   Round 2: cw={3,2,2}  -> pick A(3), cw={-4,2,2}
     *   Round 3: cw={1,3,3}  -> pick B(3), cw={1,-4,3}
     *   Round 4: cw={6,-3,4} -> pick A(6), cw={-1,-3,4}
     *   Round 5: cw={4,-2,5} -> pick C(5), cw={4,-2,-2}
     *   Round 6: cw={9,-1,-1}-> pick A(9), cw={2,-1,-1}
     *   Round 7: cw={7,0,0}  -> pick A(7), cw={0,0,0}
     */
    @Test
    void testSmoothDistribution() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");
        Node nodeB = fastBuild(cluster, "10.0.0.2");
        Node nodeC = fastBuild(cluster, "10.0.0.3");

        wrr.setWeight(nodeA, 5);
        wrr.setWeight(nodeB, 1);
        wrr.setWeight(nodeC, 1);

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        // Collect one full cycle (totalWeight = 7 selections)
        String[] expected = {"10.0.0.1", "10.0.0.1", "10.0.0.2", "10.0.0.1", "10.0.0.3", "10.0.0.1", "10.0.0.1"};
        String[] actual = new String[7];

        for (int i = 0; i < 7; i++) {
            actual[i] = cluster.nextNode(request).node().socketAddress().getHostString();
        }

        // Verify the exact smooth interleaved sequence
        for (int i = 0; i < 7; i++) {
            assertEquals(expected[i], actual[i],
                    "Mismatch at position " + i + ": expected " + expected[i] + " but got " + actual[i]);
        }

        // Verify the sequence is NOT clustered: A should not appear 5 times consecutively
        int maxConsecutiveA = 0;
        int currentConsecutiveA = 0;
        for (String s : actual) {
            if ("10.0.0.1".equals(s)) {
                currentConsecutiveA++;
                maxConsecutiveA = Math.max(maxConsecutiveA, currentConsecutiveA);
            } else {
                currentConsecutiveA = 0;
            }
        }
        assertTrue(maxConsecutiveA <= 2,
                "A should never appear more than 2 times consecutively in smooth WRR with weights {5,1,1}, " +
                        "but appeared " + maxConsecutiveA + " times consecutively");

        cluster.close();
    }

    /**
     * Over many iterations, the distribution should be proportional to the
     * configured weights. With weights {3,2,1}, node A should get 50%,
     * node B 33.3%, and node C 16.7%.
     */
    @Test
    void testWeightProportionality() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");
        Node nodeB = fastBuild(cluster, "10.0.0.2");
        Node nodeC = fastBuild(cluster, "10.0.0.3");

        wrr.setWeight(nodeA, 3);
        wrr.setWeight(nodeB, 2);
        wrr.setWeight(nodeC, 1);

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        // Run a multiple of totalWeight (6) to get exact proportionality
        int cycles = 1000;
        int totalWeight = 6;
        int totalSelections = cycles * totalWeight;

        Map<String, Integer> counts = new HashMap<>();
        for (int i = 0; i < totalSelections; i++) {
            Node node = cluster.nextNode(request).node();
            counts.merge(node.socketAddress().getHostString(), 1, Integer::sum);
        }

        // Smooth WRR is deterministic: over exact multiples of totalWeight, counts are exact
        assertEquals(cycles * 3, counts.get("10.0.0.1"), "Node A (weight=3) should get 3/6 of selections");
        assertEquals(cycles * 2, counts.get("10.0.0.2"), "Node B (weight=2) should get 2/6 of selections");
        assertEquals(cycles * 1, counts.get("10.0.0.3"), "Node C (weight=1) should get 1/6 of selections");

        cluster.close();
    }

    /**
     * Calling setWeight() on a node should change its effective weight and
     * take effect on the next selection cycle.
     */
    @Test
    void testDynamicWeightChange() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");
        Node nodeB = fastBuild(cluster, "10.0.0.2");

        // Start with equal weights (default = 1 each)
        assertEquals(1, wrr.getWeight(nodeA));
        assertEquals(1, wrr.getWeight(nodeB));

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        // With equal weights, verify roughly equal distribution over one cycle
        Map<String, Integer> countsBefore = new HashMap<>();
        int cycleSize = 100; // Multiple of totalWeight=2
        for (int i = 0; i < cycleSize; i++) {
            Node node = cluster.nextNode(request).node();
            countsBefore.merge(node.socketAddress().getHostString(), 1, Integer::sum);
        }
        assertEquals(50, countsBefore.get("10.0.0.1"));
        assertEquals(50, countsBefore.get("10.0.0.2"));

        // Now change weight of A to 9, B remains 1. totalWeight = 10
        wrr.setWeight(nodeA, 9);
        assertEquals(9, wrr.getWeight(nodeA));
        assertEquals(1, wrr.getWeight(nodeB));

        // Run 10 full cycles (100 selections, multiple of totalWeight=10)
        Map<String, Integer> countsAfter = new HashMap<>();
        int selectionsAfter = 100;
        for (int i = 0; i < selectionsAfter; i++) {
            Node node = cluster.nextNode(request).node();
            countsAfter.merge(node.socketAddress().getHostString(), 1, Integer::sum);
        }

        // After weight change: A should get 90%, B should get 10%
        assertEquals(90, countsAfter.get("10.0.0.1"), "Node A (weight=9) should get 9/10 of selections");
        assertEquals(10, countsAfter.get("10.0.0.2"), "Node B (weight=1) should get 1/10 of selections");

        cluster.close();
    }

    /**
     * When a node is removed from the cluster, its load should redistribute
     * to the remaining nodes proportional to their weights.
     */
    @Test
    void testNodeRemoval() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");
        Node nodeB = fastBuild(cluster, "10.0.0.2");
        Node nodeC = fastBuild(cluster, "10.0.0.3");

        wrr.setWeight(nodeA, 2);
        wrr.setWeight(nodeB, 2);
        wrr.setWeight(nodeC, 1);

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        // Verify all three nodes participate before removal
        Map<String, Integer> countsBefore = new HashMap<>();
        for (int i = 0; i < 50; i++) {
            Node node = cluster.nextNode(request).node();
            countsBefore.merge(node.socketAddress().getHostString(), 1, Integer::sum);
        }
        assertEquals(3, countsBefore.size(), "All 3 nodes should receive traffic");

        // Remove node C. Node.close() sets state to OFFLINE and removes from cluster,
        // which publishes NodeRemovedTask, cleaning up the weight map.
        nodeC.close();

        // After removal, only A and B remain with weights {2, 2}
        Map<String, Integer> countsAfter = new HashMap<>();
        int selectionsAfter = 100;
        for (int i = 0; i < selectionsAfter; i++) {
            Node node = cluster.nextNode(request).node();
            countsAfter.merge(node.socketAddress().getHostString(), 1, Integer::sum);
        }

        assertEquals(2, countsAfter.size(), "Only 2 nodes should receive traffic after removal");
        assertEquals(50, countsAfter.get("10.0.0.1"), "Node A (weight=2) should get half");
        assertEquals(50, countsAfter.get("10.0.0.2"), "Node B (weight=2) should get half");
        // Removed node should not appear
        assertTrue(!countsAfter.containsKey("10.0.0.3"),
                "Removed node C should not receive any traffic");

        cluster.close();
    }

    /**
     * Adding a new node to the cluster should give it the default weight (1)
     * and it should immediately participate in load balancing.
     */
    @Test
    void testNodeAddition() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");
        wrr.setWeight(nodeA, 3);

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        // With only A, all selections go to A
        for (int i = 0; i < 5; i++) {
            assertEquals("10.0.0.1", cluster.nextNode(request).node().socketAddress().getHostString());
        }

        // Add node B with default weight. totalWeight becomes 3 + 1 = 4
        Node nodeB = fastBuild(cluster, "10.0.0.2");
        assertEquals(1, wrr.getWeight(nodeB), "New node should have default weight of 1");

        // Over 4 selections (one full cycle), A should get 3 and B should get 1
        Map<String, Integer> counts = new HashMap<>();
        int selectionsPerCycle = 4;
        int cycles = 100;
        for (int i = 0; i < selectionsPerCycle * cycles; i++) {
            Node node = cluster.nextNode(request).node();
            counts.merge(node.socketAddress().getHostString(), 1, Integer::sum);
        }

        assertEquals(300, counts.get("10.0.0.1"), "Node A (weight=3) should get 3/4 of selections");
        assertEquals(100, counts.get("10.0.0.2"), "Node B (weight=1) should get 1/4 of selections");

        cluster.close();
    }

    /**
     * With a single node, every selection should return that node regardless
     * of its weight setting.
     */
    @Test
    void testSingleNode() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");
        wrr.setWeight(nodeA, 5);

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        for (int i = 0; i < 100; i++) {
            Node selected = cluster.nextNode(request).node();
            assertNotNull(selected);
            assertEquals("10.0.0.1", selected.socketAddress().getHostString());
        }

        cluster.close();
    }

    /**
     * Calling response() on a cluster with no online nodes should throw
     * NoNodeAvailableException.
     */
    @Test
    void testEmptyClusterThrows() {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        assertThrows(NoNodeAvailableException.class, () -> cluster.nextNode(request));
    }

    /**
     * setWeight() with a weight less than 1 should throw IllegalArgumentException.
     * Tests boundary values: 0, -1, and Integer.MIN_VALUE.
     */
    @Test
    void testInvalidWeightThrows() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");

        // Weight of 0 should throw
        assertThrows(IllegalArgumentException.class, () -> wrr.setWeight(nodeA, 0),
                "Weight 0 should be rejected");

        // Negative weight should throw
        assertThrows(IllegalArgumentException.class, () -> wrr.setWeight(nodeA, -1),
                "Negative weight should be rejected");

        // Extreme negative weight should throw
        assertThrows(IllegalArgumentException.class, () -> wrr.setWeight(nodeA, Integer.MIN_VALUE),
                "Integer.MIN_VALUE weight should be rejected");

        // Weight of 1 should succeed (boundary: minimum valid weight)
        wrr.setWeight(nodeA, 1);
        assertEquals(1, wrr.getWeight(nodeA));

        // Large positive weight should succeed
        wrr.setWeight(nodeA, 10000);
        assertEquals(10000, wrr.getWeight(nodeA));

        cluster.close();
    }

    /**
     * Verifies that concurrent calls to response() do not corrupt the internal
     * weight state. Because the algorithm maintains mutable currentWeight
     * across all nodes, concurrent unsynchronized access would produce
     * incorrect distributions or exceptions.
     *
     * This test spawns multiple threads that simultaneously call response()
     * and verifies that no exceptions are thrown and all responses are valid.
     */
    @Test
    void testConcurrentAccess() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");
        Node nodeB = fastBuild(cluster, "10.0.0.2");
        Node nodeC = fastBuild(cluster, "10.0.0.3");

        wrr.setWeight(nodeA, 5);
        wrr.setWeight(nodeB, 3);
        wrr.setWeight(nodeC, 2);

        int threadCount = 8;
        int iterationsPerThread = 5_000;
        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        List<Throwable> errors = new CopyOnWriteArrayList<>();
        AtomicInteger successCount = new AtomicInteger(0);

        // Per-host counters for validating aggregate distribution
        Map<String, AtomicInteger> globalCounts = new HashMap<>();
        globalCounts.put("10.0.0.1", new AtomicInteger(0));
        globalCounts.put("10.0.0.2", new AtomicInteger(0));
        globalCounts.put("10.0.0.3", new AtomicInteger(0));

        for (int t = 0; t < threadCount; t++) {
            final int threadId = t;
            executor.submit(() -> {
                try {
                    startLatch.await();

                    for (int i = 0; i < iterationsPerThread; i++) {
                        L4Request request = new L4Request(
                                new InetSocketAddress("192.168." + threadId + "." + (i % 256), 1));
                        var response = cluster.nextNode(request);
                        assertNotNull(response, "Response must not be null");
                        assertNotNull(response.node(), "Response node must not be null");

                        String host = response.node().socketAddress().getHostString();
                        AtomicInteger counter = globalCounts.get(host);
                        assertNotNull(counter, "Unexpected host: " + host);
                        counter.incrementAndGet();

                        successCount.incrementAndGet();
                    }
                } catch (Throwable e) {
                    errors.add(e);
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        // Release all threads simultaneously for maximum contention
        startLatch.countDown();
        assertTrue(doneLatch.await(30, TimeUnit.SECONDS), "Test timed out");
        executor.shutdown();

        // Verify no errors occurred
        assertTrue(errors.isEmpty(),
                "Concurrent WeightedRoundRobin produced errors: " + errors);

        int totalExpected = threadCount * iterationsPerThread;
        assertEquals(totalExpected, successCount.get(),
                "All iterations must complete successfully");

        // Verify total counts add up (no lost or phantom selections)
        int totalCounted = globalCounts.values().stream().mapToInt(AtomicInteger::get).sum();
        assertEquals(totalExpected, totalCounted,
                "Sum of per-host counts must equal total selections");

        // Verify distribution is roughly proportional to weights {5,3,2}.
        // Under contention the exact ratio may shift slightly because the
        // synchronized block serializes access, but the proportions should
        // be within a reasonable tolerance.
        int countA = globalCounts.get("10.0.0.1").get();
        int countB = globalCounts.get("10.0.0.2").get();
        int countC = globalCounts.get("10.0.0.3").get();

        // Expected ratios: A=50%, B=30%, C=20%
        // Allow 1% tolerance since the algorithm is deterministic even under contention
        // (synchronized ensures serial execution of the selection logic)
        double ratioA = (double) countA / totalExpected;
        double ratioB = (double) countB / totalExpected;
        double ratioC = (double) countC / totalExpected;

        assertEquals(0.50, ratioA, 0.01, "Node A (weight=5) should get ~50%, got " + ratioA);
        assertEquals(0.30, ratioB, 0.01, "Node B (weight=3) should get ~30%, got " + ratioB);
        assertEquals(0.20, ratioC, 0.01, "Node C (weight=2) should get ~20%, got " + ratioC);

        cluster.close();
    }

    /**
     * Calling close() on the WeightedRoundRobin instance should clear the
     * internal weight map and session persistence. After close, getWeight()
     * should return the default weight (1) since the tracked state is gone.
     */
    @Test
    void testCloseCleanup() throws Exception {
        WeightedRoundRobin wrr = new WeightedRoundRobin(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(wrr).build();

        Node nodeA = fastBuild(cluster, "10.0.0.1");
        Node nodeB = fastBuild(cluster, "10.0.0.2");

        wrr.setWeight(nodeA, 10);
        wrr.setWeight(nodeB, 5);

        // Verify weights are tracked
        assertEquals(10, wrr.getWeight(nodeA));
        assertEquals(5, wrr.getWeight(nodeB));

        // Close the load balancer
        wrr.close();

        // After close, getWeight() should return DEFAULT_WEIGHT (1)
        // because the weight map has been cleared.
        assertEquals(1, wrr.getWeight(nodeA),
                "After close(), getWeight should return default weight since map is cleared");
        assertEquals(1, wrr.getWeight(nodeB),
                "After close(), getWeight should return default weight since map is cleared");

        cluster.close();
    }

    private static Node fastBuild(Cluster cluster, String host) throws Exception {
        return NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
