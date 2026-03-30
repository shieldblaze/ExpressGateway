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
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link ResponseTimeTracker} covering EWMA calculation correctness,
 * cold start threshold, thread-safety of the CAS loop, and state management.
 */
class ResponseTimeTrackerTest {

    private Cluster cluster;
    private Node node1;
    private Node node2;

    @BeforeEach
    void setUp() throws Exception {
        cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();
        node1 = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();
        node2 = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.2", 8080))
                .build();
    }

    @AfterEach
    void tearDown() throws Exception {
        cluster.close();
    }

    // -----------------------------------------------------------------------
    // 1. First sample initialization
    // -----------------------------------------------------------------------

    @Test
    void testFirstSampleSetsEwma() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, 100);
        assertEquals(100.0, tracker.getEwma(node1), 0.001,
                "First sample must initialize the EWMA directly");
        assertEquals(1, tracker.getSampleCount(node1));
    }

    @Test
    void testFirstSampleZero() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, 0);
        assertEquals(0.0, tracker.getEwma(node1), 0.001,
                "A zero response time is a valid first sample");
        assertEquals(1, tracker.getSampleCount(node1));
    }

    // -----------------------------------------------------------------------
    // 2. EWMA convergence with default alpha (0.5)
    // -----------------------------------------------------------------------

    @Test
    void testEwmaConverges() {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);

        // First sample: EWMA = 100
        tracker.record(node1, 100);
        assertEquals(100.0, tracker.getEwma(node1), 0.001);

        // Second sample (200): EWMA = 0.5 * 200 + 0.5 * 100 = 150
        tracker.record(node1, 200);
        assertEquals(150.0, tracker.getEwma(node1), 0.001);

        // Third sample (200): EWMA = 0.5 * 200 + 0.5 * 150 = 175
        tracker.record(node1, 200);
        assertEquals(175.0, tracker.getEwma(node1), 0.001);

        // Fourth sample (200): EWMA = 0.5 * 200 + 0.5 * 175 = 187.5
        tracker.record(node1, 200);
        assertEquals(187.5, tracker.getEwma(node1), 0.001);
    }

    @Test
    void testEwmaConvergesToStableValue() {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);
        tracker.record(node1, 100);

        // After many identical samples, EWMA should converge to the sample value
        for (int i = 0; i < 50; i++) {
            tracker.record(node1, 200);
        }
        assertEquals(200.0, tracker.getEwma(node1), 0.01,
                "EWMA should converge to 200 after many identical samples");
    }

    // -----------------------------------------------------------------------
    // 3. Custom alpha behavior
    // -----------------------------------------------------------------------

    @Test
    void testCustomAlphaOne() {
        // alpha = 1.0 means EWMA = latest sample (no smoothing)
        ResponseTimeTracker tracker = new ResponseTimeTracker(1.0);
        tracker.record(node1, 100);
        assertEquals(100.0, tracker.getEwma(node1), 0.001);

        tracker.record(node1, 200);
        assertEquals(200.0, tracker.getEwma(node1), 0.001,
                "alpha=1.0: EWMA should equal the latest sample");

        tracker.record(node1, 50);
        assertEquals(50.0, tracker.getEwma(node1), 0.001,
                "alpha=1.0: EWMA should equal the latest sample");
    }

    @Test
    void testCustomAlphaLow() {
        // alpha close to 0 means slow change (heavy smoothing)
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.1);
        tracker.record(node1, 100);
        assertEquals(100.0, tracker.getEwma(node1), 0.001);

        // Second sample (200): EWMA = 0.1 * 200 + 0.9 * 100 = 110
        tracker.record(node1, 200);
        assertEquals(110.0, tracker.getEwma(node1), 0.001,
                "alpha=0.1: EWMA should change slowly");

        // Third sample (200): EWMA = 0.1 * 200 + 0.9 * 110 = 119
        tracker.record(node1, 200);
        assertEquals(119.0, tracker.getEwma(node1), 0.001);
    }

    @Test
    void testCustomAlphaHigh() {
        // alpha = 0.9 means fast adaptation
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.9);
        tracker.record(node1, 100);

        // Second sample (200): EWMA = 0.9 * 200 + 0.1 * 100 = 190
        tracker.record(node1, 200);
        assertEquals(190.0, tracker.getEwma(node1), 0.001,
                "alpha=0.9: EWMA should adapt quickly to new samples");
    }

    // -----------------------------------------------------------------------
    // 4. Invalid alpha values
    // -----------------------------------------------------------------------

    @Test
    void testInvalidAlphaThrows() {
        assertThrows(IllegalArgumentException.class, () -> new ResponseTimeTracker(0.0),
                "alpha=0.0 must be rejected (lower bound exclusive)");
        assertThrows(IllegalArgumentException.class, () -> new ResponseTimeTracker(-0.1),
                "Negative alpha must be rejected");
        assertThrows(IllegalArgumentException.class, () -> new ResponseTimeTracker(-1.0),
                "Negative alpha must be rejected");
        assertThrows(IllegalArgumentException.class, () -> new ResponseTimeTracker(1.1),
                "alpha > 1.0 must be rejected");
    }

    @Test
    void testAlphaBoundaryValid() {
        // alpha = 1.0 is the upper boundary and must be accepted
        ResponseTimeTracker tracker = new ResponseTimeTracker(1.0);
        tracker.record(node1, 42);
        assertEquals(42.0, tracker.getEwma(node1), 0.001);

        // alpha just above 0.0 must be accepted
        ResponseTimeTracker tracker2 = new ResponseTimeTracker(Double.MIN_VALUE);
        tracker2.record(node1, 100);
        assertEquals(100.0, tracker2.getEwma(node1), 0.001);
    }

    // -----------------------------------------------------------------------
    // 5. Cold start threshold
    // -----------------------------------------------------------------------

    @Test
    void testColdStartThreshold() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        assertFalse(tracker.isWarm(node1), "Node with 0 samples should be cold");

        // Record (COLD_START_THRESHOLD - 1) samples: node should remain cold
        for (int i = 1; i < ResponseTimeTracker.COLD_START_THRESHOLD; i++) {
            tracker.record(node1, 100);
            assertFalse(tracker.isWarm(node1),
                    "Node with " + i + " samples should still be cold (threshold=" +
                            ResponseTimeTracker.COLD_START_THRESHOLD + ")");
        }

        // The threshold-th sample should flip to warm
        tracker.record(node1, 100);
        assertTrue(tracker.isWarm(node1),
                "Node should be warm after " + ResponseTimeTracker.COLD_START_THRESHOLD + " samples");
        assertEquals(ResponseTimeTracker.COLD_START_THRESHOLD, tracker.getSampleCount(node1));
    }

    @Test
    void testColdStartThresholdIsExactlyTen() {
        assertEquals(10, ResponseTimeTracker.COLD_START_THRESHOLD,
                "Cold start threshold must be 10 as specified");
    }

    @Test
    void testIsWarmUnknownNode() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        assertFalse(tracker.isWarm(node1), "Unknown node must be reported as cold");
    }

    // -----------------------------------------------------------------------
    // 6. Negative response time handling
    // -----------------------------------------------------------------------

    @Test
    void testNegativeResponseTimeIgnored() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, 100);
        tracker.record(node1, -50);

        assertEquals(100.0, tracker.getEwma(node1), 0.001,
                "Negative response time must not affect EWMA");
        assertEquals(1, tracker.getSampleCount(node1),
                "Negative response time must not increment sample count");
    }

    @Test
    void testNegativeResponseTimeOnlyRecords() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, -1);
        tracker.record(node1, -100);
        tracker.record(node1, Long.MIN_VALUE);

        assertEquals(0.0, tracker.getEwma(node1), 0.001,
                "All negative: EWMA should remain at default 0.0");
        assertEquals(0, tracker.getSampleCount(node1),
                "All negative: sample count should remain 0");
    }

    // -----------------------------------------------------------------------
    // 7. Concurrent updates (CAS correctness)
    // -----------------------------------------------------------------------

    @Test
    void testConcurrentUpdates() throws Exception {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);
        int threadCount = 8;
        int iterationsPerThread = 5_000;

        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        List<Throwable> errors = new CopyOnWriteArrayList<>();

        for (int t = 0; t < threadCount; t++) {
            executor.submit(() -> {
                try {
                    startLatch.await();
                    for (int i = 0; i < iterationsPerThread; i++) {
                        // Samples in the range [100, 149]
                        tracker.record(node1, 100 + (i % 50));
                    }
                } catch (Throwable e) {
                    errors.add(e);
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        startLatch.countDown();
        assertTrue(doneLatch.await(30, TimeUnit.SECONDS), "Test timed out");
        executor.shutdown();

        assertTrue(errors.isEmpty(), "Concurrent updates produced errors: " + errors);

        // EWMA should be in a reasonable range for samples in [100, 149]
        double ewma = tracker.getEwma(node1);
        assertTrue(ewma > 50 && ewma < 200,
                "EWMA should be in [50, 200] range after many samples around 100-149, got: " + ewma);

        // Sample count should be approximately (threadCount * iterationsPerThread).
        // The CAS on sampleCount is separate from the CAS on ewmaBits, so under high
        // contention the count read in record() can race; allow a small tolerance.
        int totalExpected = threadCount * iterationsPerThread;
        int actualCount = tracker.getSampleCount(node1);
        assertTrue(actualCount >= totalExpected - threadCount && actualCount <= totalExpected,
                "Sample count should be approximately " + totalExpected + ", got: " + actualCount);
    }

    @Test
    void testConcurrentUpdatesMultipleNodes() throws Exception {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);
        int threadCount = 4;
        int iterationsPerThread = 2_000;

        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        List<Throwable> errors = new CopyOnWriteArrayList<>();

        // Half the threads write to node1, the other half to node2
        for (int t = 0; t < threadCount; t++) {
            final Node target = (t % 2 == 0) ? node1 : node2;
            executor.submit(() -> {
                try {
                    startLatch.await();
                    for (int i = 0; i < iterationsPerThread; i++) {
                        tracker.record(target, 100);
                    }
                } catch (Throwable e) {
                    errors.add(e);
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        startLatch.countDown();
        assertTrue(doneLatch.await(30, TimeUnit.SECONDS), "Test timed out");
        executor.shutdown();

        assertTrue(errors.isEmpty(), "Concurrent multi-node updates produced errors: " + errors);

        // Both nodes should have valid EWMA values
        assertEquals(100.0, tracker.getEwma(node1), 1.0,
                "node1 EWMA should converge to 100");
        assertEquals(100.0, tracker.getEwma(node2), 1.0,
                "node2 EWMA should converge to 100");
    }

    // -----------------------------------------------------------------------
    // 8. Remove and clear state management
    // -----------------------------------------------------------------------

    @Test
    void testRemoveNode() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, 100);
        tracker.record(node2, 200);

        tracker.remove(node1);

        assertEquals(0.0, tracker.getEwma(node1),
                "Removed node's EWMA should return 0.0");
        assertEquals(0, tracker.getSampleCount(node1),
                "Removed node's sample count should return 0");
        assertFalse(tracker.isWarm(node1),
                "Removed node should not be warm");

        // node2 should be unaffected
        assertEquals(200.0, tracker.getEwma(node2), 0.001,
                "Non-removed node should be unaffected");
    }

    @Test
    void testRemoveUnknownNodeIsNoOp() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        // Should not throw
        tracker.remove(node1);
        assertEquals(0.0, tracker.getEwma(node1));
    }

    @Test
    void testClear() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, 100);
        tracker.record(node2, 200);

        tracker.clear();

        assertEquals(0.0, tracker.getEwma(node1), "EWMA of node1 should be 0 after clear");
        assertEquals(0.0, tracker.getEwma(node2), "EWMA of node2 should be 0 after clear");
        assertEquals(0, tracker.getSampleCount(node1));
        assertEquals(0, tracker.getSampleCount(node2));
        assertFalse(tracker.isWarm(node1));
        assertFalse(tracker.isWarm(node2));
    }

    @Test
    void testRecordAfterRemove() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, 100);
        tracker.remove(node1);

        // Recording again after removal should start fresh
        tracker.record(node1, 50);
        assertEquals(50.0, tracker.getEwma(node1), 0.001,
                "Recording after remove should treat as first sample");
        assertEquals(1, tracker.getSampleCount(node1));
    }

    @Test
    void testRecordAfterClear() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, 100);
        tracker.clear();

        tracker.record(node1, 75);
        assertEquals(75.0, tracker.getEwma(node1), 0.001,
                "Recording after clear should treat as first sample");
        assertEquals(1, tracker.getSampleCount(node1));
    }

    // -----------------------------------------------------------------------
    // Additional edge cases
    // -----------------------------------------------------------------------

    @Test
    void testGetEwmaUnknownNodeReturnsZero() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        assertEquals(0.0, tracker.getEwma(node1),
                "Unknown node should return 0.0 EWMA");
    }

    @Test
    void testGetSampleCountUnknownNodeReturnsZero() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        assertEquals(0, tracker.getSampleCount(node1),
                "Unknown node should return 0 sample count");
    }

    @Test
    void testMultipleNodesIndependent() {
        ResponseTimeTracker tracker = new ResponseTimeTracker(0.5);
        tracker.record(node1, 100);
        tracker.record(node2, 500);

        assertEquals(100.0, tracker.getEwma(node1), 0.001,
                "node1 EWMA must be independent of node2");
        assertEquals(500.0, tracker.getEwma(node2), 0.001,
                "node2 EWMA must be independent of node1");
        assertEquals(1, tracker.getSampleCount(node1));
        assertEquals(1, tracker.getSampleCount(node2));
    }

    @Test
    void testDefaultAlphaIsHalf() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, 100);
        tracker.record(node1, 200);
        // 0.5 * 200 + 0.5 * 100 = 150
        assertEquals(150.0, tracker.getEwma(node1), 0.001,
                "Default constructor should use alpha=0.5");
    }

    @Test
    void testLargeResponseTime() {
        ResponseTimeTracker tracker = new ResponseTimeTracker();
        tracker.record(node1, Long.MAX_VALUE);
        assertEquals((double) Long.MAX_VALUE, tracker.getEwma(node1), 1.0,
                "Large response time should be recorded without overflow");
        assertEquals(1, tracker.getSampleCount(node1));
    }
}
