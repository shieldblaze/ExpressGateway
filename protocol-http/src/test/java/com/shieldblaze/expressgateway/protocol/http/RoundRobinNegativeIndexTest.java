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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.common.algo.roundrobin.RoundRobinIndexGenerator;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests that {@link RoundRobinIndexGenerator#decMaxIndex()} does not drive
 * {@code maxIndex} negative, which was the root cause of BUG-012.
 *
 * <p>Background: When nodes are rapidly removed from a cluster, the load
 * balancing event handler calls {@code decMaxIndex()} for each removal. If
 * {@code decMaxIndex()} is called more times than {@code incMaxIndex()} was
 * called (which can happen during rapid node churn), the old code would
 * decrement {@code maxIndex} below zero.</p>
 *
 * <p>When {@code maxIndex} reaches {@code Integer.MIN_VALUE},
 * {@code Math.abs(Integer.MIN_VALUE)} returns {@code Integer.MIN_VALUE}
 * (negative due to two's complement overflow), and the modulo operation
 * produces a negative index, causing {@code IndexOutOfBoundsException}
 * in {@code onlineNodes.get()}.</p>
 *
 * <p>The fix uses {@code Math.max(0, operand - 1)} in the
 * {@code updateAndGet()} lambda to floor the value at zero.</p>
 */
class RoundRobinNegativeIndexTest {

    /**
     * Verifies that calling decMaxIndex() more times than incMaxIndex()
     * does not produce a negative maxIndex.
     */
    @Test
    void decMaxIndex_moreThanInc_maxIndexNeverNegative() {
        RoundRobinIndexGenerator generator = new RoundRobinIndexGenerator(0);

        // Increment 5 times (simulating 5 nodes added)
        for (int i = 0; i < 5; i++) {
            generator.incMaxIndex();
        }
        assertEquals(5, generator.maxIndex().get(),
                "After 5 increments, maxIndex should be 5");

        // Decrement 10 times (more than increments -- simulates rapid churn)
        for (int i = 0; i < 10; i++) {
            generator.decMaxIndex();
        }

        // maxIndex must never go negative
        assertTrue(generator.maxIndex().get() >= 0,
                "maxIndex must never be negative, got: " + generator.maxIndex().get());
        assertEquals(0, generator.maxIndex().get(),
                "maxIndex should be floored at 0");
    }

    /**
     * Verifies that next() returns -1 (sentinel for "no nodes") when
     * maxIndex is 0, rather than throwing ArithmeticException from
     * division by zero or returning a negative index.
     */
    @Test
    void next_whenMaxIndexIsZero_returnsNegativeOne() {
        RoundRobinIndexGenerator generator = new RoundRobinIndexGenerator(0);

        // With maxIndex=0, next() should return -1 (no valid index)
        assertEquals(-1, generator.next(),
                "next() must return -1 when maxIndex is 0 (no nodes available)");

        // Even after many calls, it should consistently return -1
        for (int i = 0; i < 100; i++) {
            assertEquals(-1, generator.next(),
                    "next() must consistently return -1 when maxIndex is 0");
        }
    }

    /**
     * Verifies that next() always returns a valid index (0 <= index < maxIndex)
     * when maxIndex is positive.
     */
    @Test
    void next_withPositiveMaxIndex_alwaysReturnsValidIndex() {
        int nodeCount = 5;
        RoundRobinIndexGenerator generator = new RoundRobinIndexGenerator(nodeCount);

        // Make many calls and verify all indices are in range
        for (int i = 0; i < 1000; i++) {
            int index = generator.next();
            assertTrue(index >= 0 && index < nodeCount,
                    "Index must be in [0, " + nodeCount + ") but got: " + index);
        }
    }

    /**
     * Verifies that interleaving inc/dec operations never produces a
     * negative maxIndex, even with rapid alternation.
     */
    @Test
    void interleavedIncDec_maxIndexNeverNegative() {
        RoundRobinIndexGenerator generator = new RoundRobinIndexGenerator(0);

        // Simulate rapid node add/remove cycles
        for (int cycle = 0; cycle < 100; cycle++) {
            // Add 3 nodes
            generator.incMaxIndex();
            generator.incMaxIndex();
            generator.incMaxIndex();

            // Remove 5 nodes (more than added -- simulates churn with stale events)
            generator.decMaxIndex();
            generator.decMaxIndex();
            generator.decMaxIndex();
            generator.decMaxIndex();
            generator.decMaxIndex();

            assertTrue(generator.maxIndex().get() >= 0,
                    "maxIndex must never be negative at cycle " + cycle +
                            ", got: " + generator.maxIndex().get());
        }
    }

    /**
     * Verifies that concurrent inc/dec operations do not produce a negative
     * maxIndex. This tests the AtomicInteger-based CAS loop under contention.
     */
    @Test
    void concurrentIncDec_maxIndexNeverNegative() throws Exception {
        RoundRobinIndexGenerator generator = new RoundRobinIndexGenerator(10);

        int threadCount = 8;
        int opsPerThread = 5_000;
        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        List<Throwable> errors = new CopyOnWriteArrayList<>();

        for (int t = 0; t < threadCount; t++) {
            final boolean isIncrementer = (t % 2 == 0);
            executor.submit(() -> {
                try {
                    startLatch.await();
                    for (int i = 0; i < opsPerThread; i++) {
                        if (isIncrementer) {
                            generator.incMaxIndex();
                        } else {
                            generator.decMaxIndex();
                        }

                        // Check maxIndex after each operation
                        int currentMax = generator.maxIndex().get();
                        if (currentMax < 0) {
                            errors.add(new AssertionError(
                                    "maxIndex went negative: " + currentMax));
                        }

                        // Also verify next() doesn't throw
                        try {
                            generator.next();
                        } catch (Exception e) {
                            errors.add(new AssertionError(
                                    "next() threw exception: " + e.getMessage(), e));
                        }
                    }
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        startLatch.countDown();
        assertTrue(doneLatch.await(30, TimeUnit.SECONDS), "Test timed out");
        executor.shutdown();

        assertTrue(errors.isEmpty(),
                "Concurrent inc/dec produced errors: " + errors);

        // Final state: maxIndex must be non-negative
        assertTrue(generator.maxIndex().get() >= 0,
                "Final maxIndex must be non-negative, got: " + generator.maxIndex().get());
    }

    /**
     * Regression test for the specific Integer.MIN_VALUE overflow case.
     * Simulates the exact sequence that could trigger the bug:
     * starting from maxIndex=1, decrementing twice would have produced -1
     * under the old code.
     */
    @Test
    void exactOverflowScenario_maxIndexFloorsAtZero() {
        RoundRobinIndexGenerator generator = new RoundRobinIndexGenerator(1);

        // First dec: 1 -> 0
        generator.decMaxIndex();
        assertEquals(0, generator.maxIndex().get(), "After first dec, maxIndex should be 0");

        // Second dec: would have been 0 -> -1 without the fix
        generator.decMaxIndex();
        assertEquals(0, generator.maxIndex().get(),
                "After second dec from 0, maxIndex must remain 0 (floor guard)");

        // next() should return -1 (no valid index), not throw
        assertEquals(-1, generator.next(),
                "next() with maxIndex=0 must return -1, not throw");

        // Third dec: still should be 0
        generator.decMaxIndex();
        assertEquals(0, generator.maxIndex().get(),
                "Repeated dec below 0 must keep maxIndex at 0");
    }
}
