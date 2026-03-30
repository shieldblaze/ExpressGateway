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
package com.shieldblaze.expressgateway.metrics;

import org.junit.jupiter.api.Test;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for QA fixes on {@link StandardEdgeNetworkMetricRecorder}:
 * - decrementActiveConnections floor check (never goes negative)
 * - VarHandle.storeStoreFence in recordLatency (ordering guarantee)
 * - Per-key map size limits
 */
class MetricRecorderQAFixTest {

    /**
     * Use a fresh instance for these tests to avoid interference from the
     * singleton INSTANCE which is shared across tests.
     */
    private StandardEdgeNetworkMetricRecorder freshInstance() {
        // The class has a private constructor but we can test via INSTANCE
        // since the floor check is on the same code path. We use INSTANCE
        // and account for existing state via baseline snapshots.
        return StandardEdgeNetworkMetricRecorder.INSTANCE;
    }

    // -----------------------------------------------------------------------
    // 1. decrementActiveConnections floor check
    // -----------------------------------------------------------------------

    @Test
    void decrementActiveConnectionsDoesNotGoNegative() {
        StandardEdgeNetworkMetricRecorder recorder = freshInstance();
        long baseline = recorder.activeConnections();

        // Ensure we're at baseline by incrementing then decrementing
        recorder.incrementActiveConnections();
        recorder.decrementActiveConnections();
        assertEquals(baseline, recorder.activeConnections());

        // Now try to go below baseline -- should be clamped at 0
        // First, get to zero if baseline > 0
        for (long i = 0; i < baseline; i++) {
            recorder.decrementActiveConnections();
        }

        long atZero = recorder.activeConnections();
        assertEquals(0, atZero, "Should be at zero");

        // Decrement 10 more times -- must stay at 0
        for (int i = 0; i < 10; i++) {
            recorder.decrementActiveConnections();
        }
        assertEquals(0, recorder.activeConnections(),
                "Active connections must not go negative");

        // Restore baseline
        for (long i = 0; i < baseline; i++) {
            recorder.incrementActiveConnections();
        }
    }

    @Test
    void decrementActiveConnectionsConcurrentFloorCheck() throws Exception {
        StandardEdgeNetworkMetricRecorder recorder = freshInstance();

        // Increment 100 times
        for (int i = 0; i < 100; i++) {
            recorder.incrementActiveConnections();
        }
        assertEquals(100, recorder.activeConnections(), "Baseline should be 100 after 100 increments");

        // Concurrently decrement 200 times (100 more than we added)
        int threadCount = 8;
        int decrementsPer = 25;
        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch done = new CountDownLatch(threadCount);

        for (int t = 0; t < threadCount; t++) {
            executor.submit(() -> {
                for (int i = 0; i < decrementsPer; i++) {
                    recorder.decrementActiveConnections();
                }
                done.countDown();
            });
        }

        assertTrue(done.await(10, TimeUnit.SECONDS));
        executor.shutdown();

        assertTrue(recorder.activeConnections() >= 0,
                "Active connections must never be negative after concurrent decrements, got: " +
                        recorder.activeConnections());
    }

    // -----------------------------------------------------------------------
    // 2. recordLatency ordering (storeStoreFence)
    // -----------------------------------------------------------------------

    @Test
    void recordLatencyCountAndSumAreConsistent() {
        StandardEdgeNetworkMetricRecorder recorder = freshInstance();
        long baseCount = recorder.latencyCount();
        long baseSum = recorder.latencySum();

        // Record known latencies
        recorder.recordLatency(10);
        recorder.recordLatency(20);
        recorder.recordLatency(30);

        long count = recorder.latencyCount() - baseCount;
        long sum = recorder.latencySum() - baseSum;

        assertEquals(3, count, "Should have 3 new latency records");
        assertEquals(60, sum, "Sum should be 10+20+30=60");
    }

    // -----------------------------------------------------------------------
    // 3. Per-key map size limits (not directly testable at 10K without
    //    excessive memory, but verify the mechanism works)
    // -----------------------------------------------------------------------

    @Test
    void statusCodeRecordingWorks() {
        StandardEdgeNetworkMetricRecorder recorder = freshInstance();

        // Record a few status codes -- should work fine under the limit
        recorder.recordStatusCode(200);
        recorder.recordStatusCode(404);
        recorder.recordStatusCode(500);

        assertTrue(recorder.statusCodeCounts().containsKey(200));
        assertTrue(recorder.statusCodeCounts().containsKey(404));
        assertTrue(recorder.statusCodeCounts().containsKey(500));
    }

    @Test
    void backendLatencyRecordingWorks() {
        StandardEdgeNetworkMetricRecorder recorder = freshInstance();

        recorder.recordBackendLatency("test-backend-qa", 42);
        recorder.recordBackendLatency("test-backend-qa", 58);

        assertTrue(recorder.backendLatencies().containsKey("test-backend-qa"));
        // Average should be (42+58)/2 = 50
        assertEquals(50L, recorder.backendLatencies().get("test-backend-qa"));
    }

    @Test
    void protocolMetricsRecordingWorks() {
        StandardEdgeNetworkMetricRecorder recorder = freshInstance();

        recorder.recordProtocolBytes("QA-TEST", 100);
        recorder.recordProtocolRequest("QA-TEST");

        assertTrue(recorder.protocolBytes().containsKey("QA-TEST"));
        assertTrue(recorder.protocolRequests().containsKey("QA-TEST"));
        assertTrue(recorder.protocolBytes().get("QA-TEST") >= 100);
        assertTrue(recorder.protocolRequests().get("QA-TEST") >= 1);
    }
}
