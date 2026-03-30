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
package com.shieldblaze.expressgateway.testing;

import org.junit.jupiter.api.Test;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class PerformanceTestHarnessTest {

    @Test
    void basicBenchmark() throws InterruptedException {
        AtomicInteger counter = new AtomicInteger();

        PerformanceTestHarness.Result result = PerformanceTestHarness.builder()
                .concurrency(1)
                .iterations(100)
                .warmupIterations(10)
                .operation(counter::incrementAndGet)
                .run();

        assertNotNull(result);
        assertEquals(100, result.totalIterations());
        assertTrue(result.totalDurationNanos() > 0);
        assertTrue(result.throughputOpsPerSec() > 0);
        assertTrue(result.p50Nanos() > 0);
        assertTrue(result.p99Nanos() >= result.p50Nanos());
        assertTrue(result.maxNanos() >= result.p99Nanos());
        assertEquals(0, result.errorCount());
    }

    @Test
    void concurrentBenchmark() throws InterruptedException {
        AtomicInteger counter = new AtomicInteger();

        PerformanceTestHarness.Result result = PerformanceTestHarness.builder()
                .concurrency(4)
                .iterations(200)
                .warmupIterations(0)
                .operation(counter::incrementAndGet)
                .run();

        assertEquals(200, result.totalIterations());
        // counter might be slightly more than 200 if threads overshoot the CAS, but
        // iterations are capped by the index check
        assertTrue(counter.get() >= 200);
    }

    @Test
    void errorsAreCounted() throws InterruptedException {
        PerformanceTestHarness.Result result = PerformanceTestHarness.builder()
                .concurrency(1)
                .iterations(50)
                .warmupIterations(0)
                .operation(() -> { throw new RuntimeException("test error"); })
                .run();

        assertEquals(50, result.totalIterations());
        assertEquals(50, result.errorCount());
    }

    @Test
    void missingOperationThrows() {
        assertThrows(IllegalStateException.class,
                () -> PerformanceTestHarness.builder().run());
    }

    @Test
    void resultToString() throws InterruptedException {
        PerformanceTestHarness.Result result = PerformanceTestHarness.builder()
                .concurrency(1)
                .iterations(10)
                .warmupIterations(0)
                .operation(() -> {})
                .run();

        String str = result.toString();
        assertTrue(str.contains("PerformanceResult"));
        assertTrue(str.contains("iterations=10"));
    }
}
