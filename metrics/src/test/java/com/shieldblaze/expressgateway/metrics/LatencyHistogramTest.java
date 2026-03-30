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

import static org.junit.jupiter.api.Assertions.*;

class LatencyHistogramTest {

    @Test
    void emptyHistogramReturnsZeros() {
        LatencyHistogram h = new LatencyHistogram();
        assertEquals(0, h.count());
        assertEquals(0, h.sum());
        assertEquals(0, h.min());
        assertEquals(0, h.max());
        assertEquals(0, h.p50());
        assertEquals(0, h.p95());
        assertEquals(0, h.p99());
        assertEquals(0, h.p999());
        assertEquals(0, h.average());
    }

    @Test
    void singleSample() {
        LatencyHistogram h = new LatencyHistogram();
        h.record(42);

        assertEquals(1, h.count());
        assertEquals(42, h.sum());
        assertEquals(42, h.min());
        assertEquals(42, h.max());
        assertEquals(42, h.p50());
        assertEquals(42, h.p99());
        assertEquals(42, h.average());
    }

    @Test
    void percentileAccuracy() {
        LatencyHistogram h = new LatencyHistogram(1024);

        // Record values 1 through 100
        for (int i = 1; i <= 100; i++) {
            h.record(i);
        }

        assertEquals(100, h.count());
        assertEquals(5050, h.sum()); // sum of 1..100
        assertEquals(1, h.min());
        assertEquals(100, h.max());

        // p50 should be around 50
        long p50 = h.p50();
        assertTrue(p50 >= 45 && p50 <= 55, "p50 should be ~50, was: " + p50);

        // p95 should be around 95
        long p95 = h.p95();
        assertTrue(p95 >= 90 && p95 <= 100, "p95 should be ~95, was: " + p95);

        // p99 should be around 99
        long p99 = h.p99();
        assertTrue(p99 >= 95 && p99 <= 100, "p99 should be ~99, was: " + p99);

        assertEquals(50, h.average());
    }

    @Test
    void concurrentRecordingAccuracy() throws InterruptedException {
        LatencyHistogram h = new LatencyHistogram();
        int numThreads = 8;
        int recordsPerThread = 10_000;
        CountDownLatch latch = new CountDownLatch(numThreads);

        try (ExecutorService executor = Executors.newFixedThreadPool(numThreads)) {
            for (int t = 0; t < numThreads; t++) {
                executor.submit(() -> {
                    for (int i = 0; i < recordsPerThread; i++) {
                        h.record(i % 100);
                    }
                    latch.countDown();
                });
            }
            latch.await();
        }

        long expectedCount = (long) numThreads * recordsPerThread;
        assertEquals(expectedCount, h.count(), "Count must be exact under contention");

        // Sum should be exact: each thread sums 0..99 repeated recordsPerThread/100 times
        long expectedSumPerThread = (long) (recordsPerThread / 100) * 4950; // sum of 0..99 = 4950
        long expectedTotalSum = numThreads * expectedSumPerThread;
        assertEquals(expectedTotalSum, h.sum(), "Sum must be exact under contention");
    }

    @Test
    void bufferSizeMustBePowerOfTwo() {
        assertThrows(IllegalArgumentException.class, () -> new LatencyHistogram(100));
        assertThrows(IllegalArgumentException.class, () -> new LatencyHistogram(0));
        assertThrows(IllegalArgumentException.class, () -> new LatencyHistogram(-1));

        // Valid
        assertDoesNotThrow(() -> new LatencyHistogram(64));
        assertDoesNotThrow(() -> new LatencyHistogram(1024));
    }

    @Test
    void circularBufferOverwrite() {
        LatencyHistogram h = new LatencyHistogram(64);

        // Record 100 values -- buffer only holds 64
        for (int i = 0; i < 100; i++) {
            h.record(i);
        }

        assertEquals(100, h.count());
        // Percentiles should be from the last 64 values (36..99)
        long p50 = h.p50();
        assertTrue(p50 >= 50, "p50 of recent values should be >= 50, was: " + p50);
    }
}
