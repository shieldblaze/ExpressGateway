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
package com.shieldblaze.expressgateway.controlplane.distribution;

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link WriteBatcher} circuit breaker and backpressure behavior.
 *
 * <p>Verifies:</p>
 * <ul>
 *   <li>maxRetries is respected -- batch is dead-lettered after exhausting retries</li>
 *   <li>maxQueueDepth rejection -- submit() throws when queue is full</li>
 *   <li>CompletableFuture completes successfully on successful flush</li>
 *   <li>CompletableFuture completes exceptionally on dead-letter</li>
 *   <li>Exponential backoff timing -- retry count increments on consecutive failures</li>
 * </ul>
 */
class WriteBatcherCircuitBreakerTest {

    private record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() { }
    }

    private WriteBatcher batcher;

    @AfterEach
    void tearDown() {
        if (batcher != null) {
            batcher.close();
            batcher = null;
        }
    }

    private ConfigMutation.Upsert upsert(String name) {
        ConfigResourceId id = new ConfigResourceId("cluster", "global", name);
        ConfigResource resource = new ConfigResource(
                id, ConfigKind.CLUSTER, new ConfigScope.Global(), 1,
                Instant.now(), Instant.now(), "admin",
                Map.of(), new TestSpec(name));
        return new ConfigMutation.Upsert(resource);
    }

    // ===========================================================================
    // Test: maxRetries is respected
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Batch is dead-lettered after maxRetries consecutive failures")
    void testMaxRetriesDeadLetter() {
        AtomicInteger callCount = new AtomicInteger(0);
        int maxRetries = 3;

        batcher = new WriteBatcher(100, batch -> {
            callCount.incrementAndGet();
            throw new RuntimeException("Simulated failure #" + callCount.get());
        }, maxRetries, 10_000);
        batcher.start();

        CompletableFuture<Void> future = batcher.submit(upsert("dead-letter-test"));

        // Flush maxRetries times -- each should fail and re-queue
        for (int i = 0; i < maxRetries; i++) {
            batcher.flushNow();
        }

        // After maxRetries, the batch should be dead-lettered
        assertTrue(future.isCompletedExceptionally(),
                "Future should complete exceptionally after " + maxRetries + " failures");

        // The callback should have been invoked exactly maxRetries times
        assertEquals(maxRetries, callCount.get(),
                "Callback should be invoked exactly maxRetries times");
    }

    // ===========================================================================
    // Test: CompletableFuture completes exceptionally on dead-letter
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Dead-lettered future contains the root cause exception")
    void testDeadLetterFutureException() {
        AtomicInteger callCount = new AtomicInteger(0);
        int maxRetries = 2;

        batcher = new WriteBatcher(100, batch -> {
            callCount.incrementAndGet();
            throw new RuntimeException("Backend failure");
        }, maxRetries, 10_000);
        batcher.start();

        CompletableFuture<Void> future = batcher.submit(upsert("exception-test"));

        for (int i = 0; i < maxRetries; i++) {
            batcher.flushNow();
        }

        assertTrue(future.isCompletedExceptionally());

        ExecutionException ex = assertThrows(ExecutionException.class, future::get);
        assertInstanceOf(RuntimeException.class, ex.getCause());
        assertTrue(ex.getCause().getMessage().contains("dead-lettered"),
                "Exception message should mention dead-lettering");
    }

    // ===========================================================================
    // Test: CompletableFuture completes successfully on success
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Future completes successfully when flush callback succeeds")
    void testFutureCompletesOnSuccess() throws Exception {
        batcher = new WriteBatcher(100, batch -> {
            // Success -- no exception
        }, 5, 10_000);
        batcher.start();

        CompletableFuture<Void> future = batcher.submit(upsert("success-test"));
        batcher.flushNow();

        // Should complete without exception
        future.get(5, TimeUnit.SECONDS);
        assertTrue(future.isDone());
        assertFalse(future.isCompletedExceptionally());
    }

    // ===========================================================================
    // Test: maxQueueDepth rejection
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("submit() throws IllegalStateException when maxQueueDepth is exceeded")
    void testMaxQueueDepthRejection() {
        int maxQueueDepth = 5;

        // Use a large batch window so the scheduler never fires during this test
        batcher = new WriteBatcher(60_000, batch -> { }, 5, maxQueueDepth);
        batcher.start();

        // Fill the queue to capacity
        for (int i = 0; i < maxQueueDepth; i++) {
            batcher.submit(upsert("queue-" + i));
        }

        // The next submit should be rejected
        IllegalStateException ex = assertThrows(IllegalStateException.class,
                () -> batcher.submit(upsert("overflow")),
                "submit() should throw when queue is at maxQueueDepth");

        assertTrue(ex.getMessage().contains("queue depth exceeded"),
                "Exception message should mention queue depth: " + ex.getMessage());
    }

    // ===========================================================================
    // Test: Exponential backoff -- retry count increments
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Retry count increments on consecutive failures, resets on success")
    void testRetryCountBehavior() {
        AtomicInteger callCount = new AtomicInteger(0);
        List<List<ConfigMutation>> successBatches = new CopyOnWriteArrayList<>();

        batcher = new WriteBatcher(100, batch -> {
            int attempt = callCount.getAndIncrement();
            if (attempt < 2) {
                throw new RuntimeException("Failure attempt " + attempt);
            }
            // Third attempt succeeds
            successBatches.add(new ArrayList<>(batch));
        }, 5, 10_000);
        batcher.start();

        CompletableFuture<Void> future = batcher.submit(upsert("retry-test"));

        // First flush: fails, re-queued (retry 1)
        batcher.flushNow();
        assertFalse(future.isDone(), "Future should not be done after first failure");

        // Second flush: fails again, re-queued (retry 2)
        batcher.flushNow();
        assertFalse(future.isDone(), "Future should not be done after second failure");

        // Third flush: succeeds
        batcher.flushNow();
        assertTrue(future.isDone(), "Future should be done after successful flush");
        assertFalse(future.isCompletedExceptionally(), "Future should not be exceptional");

        assertEquals(1, successBatches.size(), "Should have exactly one successful batch");
        assertEquals(3, callCount.get(), "Callback should have been called 3 times total");
    }

    // ===========================================================================
    // Test: Multiple mutations in a dead-lettered batch all complete exceptionally
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("All futures in a dead-lettered batch complete exceptionally")
    void testAllFuturesDeadLettered() {
        int maxRetries = 2;

        batcher = new WriteBatcher(100, batch -> {
            throw new RuntimeException("Always fail");
        }, maxRetries, 10_000);
        batcher.start();

        CompletableFuture<Void> f1 = batcher.submit(upsert("dl-a"));
        CompletableFuture<Void> f2 = batcher.submit(upsert("dl-b"));
        CompletableFuture<Void> f3 = batcher.submit(upsert("dl-c"));

        for (int i = 0; i < maxRetries; i++) {
            batcher.flushNow();
        }

        assertTrue(f1.isCompletedExceptionally(), "f1 should be dead-lettered");
        assertTrue(f2.isCompletedExceptionally(), "f2 should be dead-lettered");
        assertTrue(f3.isCompletedExceptionally(), "f3 should be dead-lettered");
    }

    // ===========================================================================
    // Test: Constructor validation
    // ===========================================================================

    @Test
    @DisplayName("Constructor rejects maxRetries < 1")
    void testInvalidMaxRetries() {
        assertThrows(IllegalArgumentException.class,
                () -> new WriteBatcher(100, batch -> { }, 0, 100));
    }

    @Test
    @DisplayName("Constructor rejects maxQueueDepth < 1")
    void testInvalidMaxQueueDepth() {
        assertThrows(IllegalArgumentException.class,
                () -> new WriteBatcher(100, batch -> { }, 1, 0));
    }
}
