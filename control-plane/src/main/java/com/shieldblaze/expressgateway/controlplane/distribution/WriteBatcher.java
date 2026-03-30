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

import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.util.AbstractMap;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentLinkedDeque;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Coalesces config mutations within a configurable time window to reduce
 * write pressure on the KV store and prevent thundering herd on nodes.
 * Inspired by Cloudflare Quicksilver's 500ms batching window.
 *
 * <p>Thread-safe: mutations can be submitted from any thread (e.g., REST handlers,
 * gRPC interceptors). The batch is drained and flushed on a single dedicated
 * scheduler thread, ensuring single-writer semantics on the downstream journal.</p>
 *
 * <p>Circuit breaker: if a batch fails {@code maxRetries} consecutive times with
 * exponential backoff (capped at 30 seconds), the batch is dead-lettered (logged
 * at ERROR and dropped) to prevent unbounded retry accumulation. Each mutation's
 * {@link CompletableFuture} is completed exceptionally when dead-lettered.</p>
 *
 * <p>Backpressure: the pending queue has a configurable depth limit
 * ({@code maxQueueDepth}). When exceeded, {@link #submit(ConfigMutation)} throws
 * {@link IllegalStateException} so callers can apply upstream backpressure.</p>
 */
@Log4j2
public final class WriteBatcher implements Closeable {

    /**
     * Functional interface for the flush callback that allows checked exceptions
     * to propagate. Unlike {@link java.util.function.Consumer}, this lets the
     * caller (e.g., {@link ConfigDistributor#onBatchFlush}) throw
     * {@link com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException}
     * so the batcher can re-queue the failed batch for retry.
     */
    @FunctionalInterface
    public interface BatchFlushCallback {
        void accept(List<ConfigMutation> batch) throws Exception;
    }

    /** Maximum exponential backoff delay in milliseconds. */
    private static final long MAX_BACKOFF_MS = 30_000;

    private final long batchWindowMs;
    private final BatchFlushCallback flushCallback;
    private final ConcurrentLinkedDeque<Map.Entry<ConfigMutation, CompletableFuture<Void>>> pendingMutations =
            new ConcurrentLinkedDeque<>();
    private final ScheduledExecutorService scheduler;
    private final int maxRetries;
    private final int maxQueueDepth;
    private final AtomicInteger queueSize = new AtomicInteger(0);
    private volatile boolean running;

    /**
     * Retry state for the current batch being retried. Only accessed from the
     * scheduler thread so no synchronization is needed.
     */
    private int currentRetryCount;
    private long currentBackoffMs;

    /**
     * Constructs a WriteBatcher with default circuit breaker settings
     * (maxRetries=5, maxQueueDepth=10000).
     *
     * @param batchWindowMs the batching window in milliseconds (e.g., 500);
     *                      mutations submitted within one window are coalesced into a single flush
     * @param flushCallback the callback invoked with each coalesced batch; must not be null.
     *                      The callback receives the batch on the scheduler thread and must not block indefinitely.
     */
    public WriteBatcher(long batchWindowMs, BatchFlushCallback flushCallback) {
        this(batchWindowMs, flushCallback, 5, 10_000);
    }

    /**
     * Constructs a WriteBatcher with explicit circuit breaker settings.
     *
     * @param batchWindowMs the batching window in milliseconds (e.g., 500)
     * @param flushCallback the callback invoked with each coalesced batch; must not be null
     * @param maxRetries    maximum number of retry attempts per batch before dead-lettering (must be >= 1)
     * @param maxQueueDepth maximum number of pending mutations before submit() rejects (must be >= 1)
     */
    public WriteBatcher(long batchWindowMs, BatchFlushCallback flushCallback, int maxRetries, int maxQueueDepth) {
        if (batchWindowMs <= 0) {
            throw new IllegalArgumentException("batchWindowMs must be > 0, got: " + batchWindowMs);
        }
        if (maxRetries < 1) {
            throw new IllegalArgumentException("maxRetries must be >= 1, got: " + maxRetries);
        }
        if (maxQueueDepth < 1) {
            throw new IllegalArgumentException("maxQueueDepth must be >= 1, got: " + maxQueueDepth);
        }
        this.batchWindowMs = batchWindowMs;
        this.flushCallback = Objects.requireNonNull(flushCallback, "flushCallback");
        this.maxRetries = maxRetries;
        this.maxQueueDepth = maxQueueDepth;
        this.scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "cp-write-batcher");
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Start the periodic flush scheduler.
     * Must be called once before mutations will be flushed.
     */
    public void start() {
        running = true;
        scheduleNextFlush(batchWindowMs);
    }

    /**
     * Schedule the next flush after the given delay. This is used instead of
     * scheduleAtFixedRate so that backoff delays are actually honored on failure.
     */
    private void scheduleNextFlush(long delayMs) {
        if (!running || scheduler.isShutdown()) {
            return;
        }
        try {
            scheduler.schedule(this::flush, delayMs, TimeUnit.MILLISECONDS);
        } catch (java.util.concurrent.RejectedExecutionException e) {
            // Scheduler shut down between our check and the schedule call - expected during close()
        }
    }

    /**
     * Submit a single mutation to be batched.
     *
     * <p>Returns a {@link CompletableFuture} that completes when the mutation has been
     * successfully flushed (committed to the journal), or completes exceptionally if
     * the batch is dead-lettered after exhausting retries.</p>
     *
     * @param mutation the mutation to enqueue; must not be null
     * @return a future that completes when the mutation is committed or dead-lettered
     * @throws IllegalStateException if the pending queue depth exceeds {@code maxQueueDepth}
     */
    public CompletableFuture<Void> submit(ConfigMutation mutation) {
        Objects.requireNonNull(mutation, "mutation");
        int currentSize = queueSize.get();
        if (currentSize >= maxQueueDepth) {
            throw new IllegalStateException(
                    "WriteBatcher queue depth exceeded: " + currentSize + " >= " + maxQueueDepth);
        }
        CompletableFuture<Void> future = new CompletableFuture<>();
        pendingMutations.add(new AbstractMap.SimpleImmutableEntry<>(mutation, future));
        queueSize.incrementAndGet();
        return future;
    }

    /**
     * Submit multiple mutations to be batched.
     *
     * <p>Returns a {@link CompletableFuture} that completes when all mutations in the
     * list have been flushed.</p>
     *
     * @param mutations the mutations to enqueue; must not be null
     * @return a future that completes when all mutations are committed
     * @throws IllegalStateException if the pending queue depth would exceed {@code maxQueueDepth}
     */
    public CompletableFuture<Void> submitAll(List<ConfigMutation> mutations) {
        Objects.requireNonNull(mutations, "mutations");
        List<CompletableFuture<Void>> futures = new ArrayList<>(mutations.size());
        for (ConfigMutation m : mutations) {
            futures.add(submit(m));
        }
        return CompletableFuture.allOf(futures.toArray(CompletableFuture[]::new));
    }

    /**
     * Drain pending mutations and invoke the flush callback.
     *
     * <p>The entire body is wrapped in {@code catch(Throwable)} to prevent
     * {@link java.util.concurrent.ScheduledExecutorService#scheduleAtFixedRate}
     * from silently killing the periodic task on an unchecked exception.</p>
     *
     * <p>On failure the batch is re-queued at the <em>head</em> of the deque
     * (via reverse-order {@code addFirst}) so that revision ordering is preserved
     * across retries. Exponential backoff is applied: the delay doubles each retry
     * attempt, capped at 30 seconds. If {@code maxRetries} is exhausted, the batch
     * is dead-lettered (logged at ERROR, futures completed exceptionally, batch dropped).</p>
     */
    private void flush() {
        if (!running) {
            return;
        }

        try {
            doFlush();
        } finally {
            // Reschedule: use backoff delay if retrying, normal batch window otherwise
            long nextDelay = currentBackoffMs > 0 ? currentBackoffMs : batchWindowMs;
            scheduleNextFlush(nextDelay);
        }
    }

    private void doFlush() {
        // Drain all pending mutations into a local batch.
        // ConcurrentLinkedDeque.poll() is lock-free and returns null when empty.
        List<Map.Entry<ConfigMutation, CompletableFuture<Void>>> batch = new ArrayList<>();
        Map.Entry<ConfigMutation, CompletableFuture<Void>> entry;
        while ((entry = pendingMutations.poll()) != null) {
            batch.add(entry);
            queueSize.decrementAndGet();
        }

        if (batch.isEmpty()) {
            return;
        }

        // Extract just the mutations for the flush callback
        List<ConfigMutation> mutations = new ArrayList<>(batch.size());
        for (Map.Entry<ConfigMutation, CompletableFuture<Void>> e : batch) {
            mutations.add(e.getKey());
        }

        log.debug("Flushing batch of {} mutations (retry {})", mutations.size(), currentRetryCount);
        try {
            flushCallback.accept(mutations);
            // Success -- complete all futures and reset retry state
            for (Map.Entry<ConfigMutation, CompletableFuture<Void>> e : batch) {
                e.getValue().complete(null);
            }
            currentRetryCount = 0;
            currentBackoffMs = 0;
        } catch (Throwable t) {
            currentRetryCount++;

            if (currentRetryCount >= maxRetries) {
                // Dead-letter: log the mutations at ERROR and drop the batch
                log.error("Batch of {} mutations dead-lettered after {} retries. Mutations: {}",
                        mutations.size(), currentRetryCount, mutations, t);
                RuntimeException deadLetterException = new RuntimeException(
                        "Batch dead-lettered after " + currentRetryCount + " retries", t);
                for (Map.Entry<ConfigMutation, CompletableFuture<Void>> e : batch) {
                    e.getValue().completeExceptionally(deadLetterException);
                }
                currentRetryCount = 0;
                currentBackoffMs = 0;
                return;
            }

            // Exponential backoff: double the delay each retry, cap at MAX_BACKOFF_MS
            currentBackoffMs = currentBackoffMs == 0
                    ? batchWindowMs
                    : Math.min(currentBackoffMs * 2, MAX_BACKOFF_MS);

            log.error("Failed to flush mutation batch of {} mutations (retry {}/{}), " +
                            "re-queuing at head for retry in {}ms",
                    mutations.size(), currentRetryCount, maxRetries, currentBackoffMs, t);

            // Re-queue failed mutations at the HEAD of the deque to preserve ordering.
            // Iterate in reverse so that successive addFirst() calls reconstruct
            // the original order at the front of the deque.
            List<Map.Entry<ConfigMutation, CompletableFuture<Void>>> reversed = new ArrayList<>(batch);
            Collections.reverse(reversed);
            for (Map.Entry<ConfigMutation, CompletableFuture<Void>> failed : reversed) {
                pendingMutations.addFirst(failed);
                queueSize.incrementAndGet();
            }
        }
    }

    /**
     * Force an immediate flush outside the normal schedule.
     * Useful for shutdown sequences and testing.
     */
    public void flushNow() {
        flush();
    }

    /**
     * Shut down the batcher gracefully.
     *
     * <p>Order of operations matters to avoid the scheduler and this method
     * racing to drain the queue simultaneously:</p>
     * <ol>
     *   <li>Stop the scheduler (no new flush tasks will be scheduled)</li>
     *   <li>Await termination of any in-flight flush</li>
     *   <li>Drain remaining mutations with a final flush</li>
     * </ol>
     */
    @Override
    public void close() {
        // 1. Prevent reschedule from in-flight flush, then stop the scheduler
        running = false;
        scheduler.shutdown();

        // 2. Wait for any in-flight flush to complete
        try {
            if (!scheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                scheduler.shutdownNow();
                scheduler.awaitTermination(2, TimeUnit.SECONDS);
            }
        } catch (InterruptedException e) {
            scheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }

        // 3. Now that no scheduler task is running, drain remaining mutations.
        doFlush();
    }
}
