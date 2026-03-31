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
package com.shieldblaze.expressgateway.core;

import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link MemoryBudget} covering acquire/release semantics, CAS thread-safety,
 * overflow protection, usage metrics, and memory size parsing.
 */
class MemoryBudgetTest {

    // -----------------------------------------------------------------------
    // 1. Basic acquire and release
    // -----------------------------------------------------------------------

    @Test
    void testBasicAcquireRelease() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertTrue(budget.tryAcquire(512));
        assertEquals(512, budget.getUsedBytes());

        budget.release(512);
        assertEquals(0, budget.getUsedBytes());
    }

    @Test
    void testMultipleAcquiresThenReleases() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertTrue(budget.tryAcquire(256));
        assertTrue(budget.tryAcquire(256));
        assertTrue(budget.tryAcquire(256));
        assertEquals(768, budget.getUsedBytes());

        budget.release(256);
        budget.release(256);
        budget.release(256);
        assertEquals(0, budget.getUsedBytes());
    }

    @Test
    void testExactBudgetSucceeds() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertTrue(budget.tryAcquire(1024), "Acquiring exact max budget should succeed");
        assertEquals(1024, budget.getUsedBytes());
        budget.release(1024);
    }

    @Test
    void testGetMaxBytes() {
        MemoryBudget budget = new MemoryBudget(2048);
        assertEquals(2048, budget.getMaxBytes());
    }

    // -----------------------------------------------------------------------
    // 2. Over-budget rejection
    // -----------------------------------------------------------------------

    @Test
    void testOverBudgetRejected() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertTrue(budget.tryAcquire(1024));
        assertFalse(budget.tryAcquire(1),
                "Any acquire beyond max must be rejected");
        assertEquals(1024, budget.getUsedBytes(),
                "Failed acquire must not change used bytes");
        budget.release(1024);
    }

    @Test
    void testOverBudgetSingleLargeAcquire() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertFalse(budget.tryAcquire(1025),
                "Acquire exceeding max in a single call must be rejected");
        assertEquals(0, budget.getUsedBytes(),
                "Failed acquire must leave used bytes at 0");
    }

    @Test
    void testOverBudgetIncrementalFill() {
        MemoryBudget budget = new MemoryBudget(100);
        for (int i = 0; i < 10; i++) {
            assertTrue(budget.tryAcquire(10));
        }
        assertFalse(budget.tryAcquire(1),
                "Budget fully consumed: additional acquire must fail");
        assertEquals(100, budget.getUsedBytes());

        // Release all
        for (int i = 0; i < 10; i++) {
            budget.release(10);
        }
        assertEquals(0, budget.getUsedBytes());
    }

    // -----------------------------------------------------------------------
    // 3. CAS contention under concurrent access
    // -----------------------------------------------------------------------

    @Test
    void testCASContention() throws Exception {
        MemoryBudget budget = new MemoryBudget(1_000_000);
        int threadCount = 8;
        int iterationsPerThread = 10_000;
        long bytesPerOp = 10;

        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        List<Throwable> errors = new CopyOnWriteArrayList<>();
        AtomicInteger acquireSuccessCount = new AtomicInteger(0);

        for (int t = 0; t < threadCount; t++) {
            executor.submit(() -> {
                try {
                    startLatch.await();
                    for (int i = 0; i < iterationsPerThread; i++) {
                        if (budget.tryAcquire(bytesPerOp)) {
                            acquireSuccessCount.incrementAndGet();
                            budget.release(bytesPerOp);
                        }
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

        assertTrue(errors.isEmpty(), "CAS contention produced errors: " + errors);
        assertEquals(0, budget.getUsedBytes(),
                "After all acquire/release pairs, used bytes must be 0");
        assertTrue(acquireSuccessCount.get() > 0, "Some acquires should have succeeded");
    }

    @Test
    void testCASContentionWithBudgetPressure() throws Exception {
        // Budget of 1 byte with 8 threads each acquiring 1 byte forces heavy contention
        // and guarantees rejections since only 1 thread can hold the budget at a time.
        MemoryBudget budget = new MemoryBudget(1);
        int threadCount = 8;
        int iterationsPerThread = 5_000;
        long bytesPerOp = 1;

        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        List<Throwable> errors = new CopyOnWriteArrayList<>();
        AtomicInteger rejectCount = new AtomicInteger(0);
        AtomicInteger successCount = new AtomicInteger(0);

        for (int t = 0; t < threadCount; t++) {
            executor.submit(() -> {
                try {
                    startLatch.await();
                    for (int i = 0; i < iterationsPerThread; i++) {
                        if (budget.tryAcquire(bytesPerOp)) {
                            successCount.incrementAndGet();
                            budget.release(bytesPerOp);
                        } else {
                            rejectCount.incrementAndGet();
                        }
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

        assertTrue(errors.isEmpty(),
                "CAS contention under pressure produced errors: " + errors);
        assertEquals(0, budget.getUsedBytes(),
                "Used bytes must be 0 after all paired acquire/release");
        assertTrue(successCount.get() > 0, "Some acquires should have succeeded");
        // With 8 threads contending for 1 byte, rejections are virtually guaranteed
        // Total iterations = 40000, max possible successes = 40000 (if perfectly serialized)
        assertEquals(threadCount * iterationsPerThread, successCount.get() + rejectCount.get(),
                "Total attempts must equal success + reject counts");
    }

    // -----------------------------------------------------------------------
    // 4. Overflow protection
    // -----------------------------------------------------------------------

    @Test
    void testOverflowProtection() {
        MemoryBudget budget = new MemoryBudget(Long.MAX_VALUE);
        assertTrue(budget.tryAcquire(Long.MAX_VALUE - 1));

        // Adding 2 more bytes would cause long overflow (wraps negative)
        assertFalse(budget.tryAcquire(2),
                "Acquire that causes long overflow must be rejected");
        assertEquals(Long.MAX_VALUE - 1, budget.getUsedBytes(),
                "Failed overflow acquire must not corrupt used bytes");

        budget.release(Long.MAX_VALUE - 1);
    }

    @Test
    void testOverflowProtectionExactMax() {
        MemoryBudget budget = new MemoryBudget(Long.MAX_VALUE);
        assertTrue(budget.tryAcquire(Long.MAX_VALUE),
                "Acquiring exactly Long.MAX_VALUE (equal to max) should succeed");
        assertFalse(budget.tryAcquire(1),
                "Additional byte after max should be rejected");
        budget.release(Long.MAX_VALUE);
    }

    // -----------------------------------------------------------------------
    // 5. Release more than acquired
    // -----------------------------------------------------------------------

    @Test
    void testReleaseMoreThanAcquiredThrows() {
        MemoryBudget budget = new MemoryBudget(1024);
        budget.tryAcquire(100);
        assertThrows(IllegalStateException.class, () -> budget.release(200),
                "Releasing more than acquired must throw IllegalStateException");
    }

    @Test
    void testReleaseWithNothingAcquiredThrows() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertThrows(IllegalStateException.class, () -> budget.release(1),
                "Releasing with zero used bytes must throw IllegalStateException");
    }

    // -----------------------------------------------------------------------
    // 6. Negative bytes argument
    // -----------------------------------------------------------------------

    @Test
    void testNegativeBytesThrows() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertThrows(IllegalArgumentException.class, () -> budget.tryAcquire(-1),
                "Negative bytes in tryAcquire must throw");
        assertThrows(IllegalArgumentException.class, () -> budget.release(-1),
                "Negative bytes in release must throw");
        assertThrows(IllegalArgumentException.class, () -> budget.tryAcquire(Long.MIN_VALUE),
                "Long.MIN_VALUE in tryAcquire must throw");
        assertThrows(IllegalArgumentException.class, () -> budget.release(Long.MIN_VALUE),
                "Long.MIN_VALUE in release must throw");
    }

    // -----------------------------------------------------------------------
    // 7. Zero bytes is a no-op
    // -----------------------------------------------------------------------

    @Test
    void testZeroBytesNoOp() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertTrue(budget.tryAcquire(0),
                "tryAcquire(0) must return true");
        assertEquals(0, budget.getUsedBytes(),
                "tryAcquire(0) must not change used bytes");

        budget.release(0); // must not throw
        assertEquals(0, budget.getUsedBytes(),
                "release(0) must not change used bytes");
    }

    @Test
    void testZeroBytesAcquireWhenFull() {
        MemoryBudget budget = new MemoryBudget(1024);
        budget.tryAcquire(1024);
        assertTrue(budget.tryAcquire(0),
                "tryAcquire(0) must succeed even when budget is full");
        budget.release(1024);
    }

    // -----------------------------------------------------------------------
    // 8. Usage ratio
    // -----------------------------------------------------------------------

    @Test
    void testUsageRatio() {
        MemoryBudget budget = new MemoryBudget(1000);
        assertEquals(0.0, budget.getUsageRatio(), 0.001,
                "Initial usage ratio must be 0.0");

        budget.tryAcquire(500);
        assertEquals(0.5, budget.getUsageRatio(), 0.001);

        budget.tryAcquire(250);
        assertEquals(0.75, budget.getUsageRatio(), 0.001);

        budget.release(750);
        assertEquals(0.0, budget.getUsageRatio(), 0.001,
                "Usage ratio must return to 0.0 after full release");
    }

    @Test
    void testUsageRatioAtCapacity() {
        MemoryBudget budget = new MemoryBudget(1000);
        budget.tryAcquire(1000);
        assertEquals(1.0, budget.getUsageRatio(), 0.001,
                "Full budget should yield ratio 1.0");
        budget.release(1000);
    }

    // -----------------------------------------------------------------------
    // 9. isOverThreshold
    // -----------------------------------------------------------------------

    @Test
    void testIsOverThreshold() {
        MemoryBudget budget = new MemoryBudget(1000);
        budget.tryAcquire(901);
        assertTrue(budget.isOverThreshold(0.9),
                "901/1000 = 90.1% should be over 90% threshold");
        assertFalse(budget.isOverThreshold(0.95),
                "901/1000 = 90.1% should NOT be over 95% threshold");
        budget.release(901);
    }

    @Test
    void testIsOverThresholdBoundaries() {
        MemoryBudget budget = new MemoryBudget(1000);
        budget.tryAcquire(500);

        assertFalse(budget.isOverThreshold(0.5),
                "500/1000 = 50%, which is equal to (not over) 0.5 threshold");
        assertTrue(budget.isOverThreshold(0.49),
                "500/1000 = 50%, which is over 49% threshold");

        // Test threshold boundaries
        assertFalse(budget.isOverThreshold(1.0),
                "50% usage should not be over 100% threshold");
        assertTrue(budget.isOverThreshold(0.0),
                "50% usage should be over 0% threshold");

        budget.release(500);
    }

    @Test
    void testIsOverThresholdAtZeroUsage() {
        MemoryBudget budget = new MemoryBudget(1000);
        assertFalse(budget.isOverThreshold(0.0),
                "0 used bytes should NOT be over 0.0 threshold (0 > 0 is false)");
        assertFalse(budget.isOverThreshold(0.5));
        assertFalse(budget.isOverThreshold(1.0));
    }

    @Test
    void testIsOverThresholdInvalidThreshold() {
        MemoryBudget budget = new MemoryBudget(1024);
        assertThrows(IllegalArgumentException.class, () -> budget.isOverThreshold(-0.1),
                "Negative threshold must be rejected");
        assertThrows(IllegalArgumentException.class, () -> budget.isOverThreshold(1.1),
                "Threshold > 1.0 must be rejected");
        assertThrows(IllegalArgumentException.class, () -> budget.isOverThreshold(-1.0));
        assertThrows(IllegalArgumentException.class, () -> budget.isOverThreshold(2.0));
    }

    // -----------------------------------------------------------------------
    // 10. parseMemorySize
    // -----------------------------------------------------------------------

    @Test
    void testParseMemorySize() {
        // Kilobytes
        assertEquals(1024L, MemoryBudget.parseMemorySize("1k"));
        assertEquals(1024L, MemoryBudget.parseMemorySize("1K"));
        assertEquals(1024L * 1024, MemoryBudget.parseMemorySize("1024k"));

        // Megabytes
        assertEquals(1024L * 1024, MemoryBudget.parseMemorySize("1m"));
        assertEquals(1024L * 1024, MemoryBudget.parseMemorySize("1M"));
        assertEquals(512L * 1024 * 1024, MemoryBudget.parseMemorySize("512m"));

        // Gigabytes
        assertEquals(1024L * 1024 * 1024, MemoryBudget.parseMemorySize("1g"));
        assertEquals(1024L * 1024 * 1024, MemoryBudget.parseMemorySize("1G"));
        assertEquals(2L * 1024 * 1024 * 1024, MemoryBudget.parseMemorySize("2g"));

        // Plain numeric (bytes)
        assertEquals(1073741824L, MemoryBudget.parseMemorySize("1073741824"));
        assertEquals(0L, MemoryBudget.parseMemorySize("0"));
        assertEquals(1L, MemoryBudget.parseMemorySize("1"));
    }

    @Test
    void testParseMemorySizeWithWhitespace() {
        assertEquals(1024L, MemoryBudget.parseMemorySize(" 1k "));
        assertEquals(1024L * 1024, MemoryBudget.parseMemorySize("  1m  "));
    }

    @Test
    void testParseMemorySizeInvalid() {
        assertEquals(-1, MemoryBudget.parseMemorySize(null),
                "null input must return -1");
        assertEquals(-1, MemoryBudget.parseMemorySize(""),
                "Empty string must return -1");
        assertEquals(-1, MemoryBudget.parseMemorySize("abc"),
                "Non-numeric must return -1");
        assertEquals(-1, MemoryBudget.parseMemorySize("1x"),
                "Unknown suffix must return -1");
        assertEquals(-1, MemoryBudget.parseMemorySize("1t"),
                "Unsupported suffix 't' must return -1");
        assertEquals(-1, MemoryBudget.parseMemorySize("m"),
                "Suffix only (no number) must return -1");
    }

    // -----------------------------------------------------------------------
    // 11. Invalid maxBytes in constructor
    // -----------------------------------------------------------------------

    @Test
    void testInvalidMaxBytesThrows() {
        assertThrows(IllegalArgumentException.class, () -> new MemoryBudget(0),
                "maxBytes=0 must be rejected");
        assertThrows(IllegalArgumentException.class, () -> new MemoryBudget(-1),
                "Negative maxBytes must be rejected");
        assertThrows(IllegalArgumentException.class, () -> new MemoryBudget(Long.MIN_VALUE),
                "Long.MIN_VALUE maxBytes must be rejected");
    }

    @Test
    void testMinimumValidMaxBytes() {
        MemoryBudget budget = new MemoryBudget(1);
        assertEquals(1, budget.getMaxBytes());
        assertTrue(budget.tryAcquire(1));
        assertFalse(budget.tryAcquire(1));
        budget.release(1);
    }

    // -----------------------------------------------------------------------
    // Additional edge cases
    // -----------------------------------------------------------------------

    @Test
    void testDefaultConstructorDoesNotThrow() {
        // The default constructor reads JVM args; verify it completes without error
        MemoryBudget budget = new MemoryBudget();
        assertTrue(budget.getMaxBytes() > 0,
                "Default constructor must produce a positive max budget");
    }

    @Test
    void testToString() {
        MemoryBudget budget = new MemoryBudget(1024);
        budget.tryAcquire(512);
        String str = budget.toString();
        assertTrue(str.contains("used=512"), "toString must include used bytes");
        assertTrue(str.contains("max=1024"), "toString must include max bytes");
        budget.release(512);
    }

    @Test
    void testAcquireAfterPartialRelease() {
        MemoryBudget budget = new MemoryBudget(100);
        assertTrue(budget.tryAcquire(80));
        assertFalse(budget.tryAcquire(30), "80 + 30 > 100");
        budget.release(50);
        assertEquals(30, budget.getUsedBytes());
        assertTrue(budget.tryAcquire(70), "30 + 70 = 100, should succeed");
        assertEquals(100, budget.getUsedBytes());
        budget.release(100);
    }
}
