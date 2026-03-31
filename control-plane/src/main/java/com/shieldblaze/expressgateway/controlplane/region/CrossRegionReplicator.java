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
package com.shieldblaze.expressgateway.controlplane.region;

import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.conflict.VectorClock;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Asynchronously replicates config mutations to remote regions using
 * bandwidth-aware batching.
 *
 * <p>Mutations are enqueued via {@link #enqueue(ConfigMutation, VectorClock)} and
 * batched together within a configurable window (default 1 second) before being
 * dispatched to all target regions. This reduces cross-region RPC overhead.</p>
 *
 * <p>The replicator attaches a {@link VectorClock} to each batch for conflict
 * detection at region boundaries. The remote region's {@code ConflictResolver}
 * determines whether to accept or reject each mutation.</p>
 *
 * <p>Thread safety: all public methods are safe for concurrent use.</p>
 */
public final class CrossRegionReplicator implements Closeable {

    private static final Logger logger = LogManager.getLogger(CrossRegionReplicator.class);

    /** Maximum number of retry attempts for failed batch replication. */
    private static final int MAX_RETRIES = 3;

    /**
     * Callback interface for sending a batch of mutations to a remote region.
     *
     * @see #CrossRegionReplicator(String, RegionManager, ReplicationCallback, long, int)
     */
    @FunctionalInterface
    public interface ReplicationCallback {
        /**
         * Sends a replication batch to the specified target region.
         *
         * @param targetRegion the region to replicate to
         * @param batch        the batch of mutations with their vector clocks
         * @return a future that completes when the remote region has acknowledged the batch
         */
        CompletableFuture<Void> replicate(String targetRegion, ReplicationBatch batch);
    }

    /**
     * A batch of mutations destined for a remote region.
     *
     * @param sourceRegion the region that originated these mutations
     * @param entries      the mutations with their associated vector clocks
     * @param batchId      a unique identifier for this batch
     * @param createdAt    when the batch was created
     */
    public record ReplicationBatch(
            String sourceRegion,
            List<ReplicationEntry> entries,
            long batchId,
            Instant createdAt
    ) {
        public ReplicationBatch {
            Objects.requireNonNull(sourceRegion, "sourceRegion");
            Objects.requireNonNull(entries, "entries");
            Objects.requireNonNull(createdAt, "createdAt");
            entries = List.copyOf(entries);
        }
    }

    /**
     * A single mutation with its vector clock for replication.
     *
     * @param mutation the config mutation
     * @param clock    the vector clock at the time of the mutation
     */
    public record ReplicationEntry(ConfigMutation mutation, VectorClock clock) {
        public ReplicationEntry {
            Objects.requireNonNull(mutation, "mutation");
            Objects.requireNonNull(clock, "clock");
        }
    }

    private final String localRegion;
    private final RegionManager regionManager;
    private final ReplicationCallback callback;
    private final long batchWindowMs;
    private final int maxBatchSize;
    private final ScheduledExecutorService scheduler;
    private final ConcurrentLinkedQueue<ReplicationEntry> pendingQueue = new ConcurrentLinkedQueue<>();
    private final AtomicLong batchIdGenerator = new AtomicLong(0);
    private final AtomicLong replicatedCount = new AtomicLong(0);
    private final AtomicLong failedCount = new AtomicLong(0);
    private final AtomicBoolean closed = new AtomicBoolean(false);
    private volatile boolean running;

    /**
     * Creates a new cross-region replicator.
     *
     * @param localRegion   the local region ID
     * @param regionManager the region manager for discovering target regions
     * @param callback      the callback for sending batches to remote regions
     * @param batchWindowMs the batching window in milliseconds
     * @param maxBatchSize  the maximum number of entries per batch
     */
    public CrossRegionReplicator(
            String localRegion,
            RegionManager regionManager,
            ReplicationCallback callback,
            long batchWindowMs,
            int maxBatchSize) {
        this.localRegion = Objects.requireNonNull(localRegion, "localRegion");
        this.regionManager = Objects.requireNonNull(regionManager, "regionManager");
        this.callback = Objects.requireNonNull(callback, "callback");
        if (batchWindowMs <= 0) {
            throw new IllegalArgumentException("batchWindowMs must be > 0");
        }
        if (maxBatchSize < 1) {
            throw new IllegalArgumentException("maxBatchSize must be >= 1");
        }
        this.batchWindowMs = batchWindowMs;
        this.maxBatchSize = maxBatchSize;
        this.scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "cross-region-replicator");
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Starts the replication scheduler.
     */
    public void start() {
        running = true;
        scheduler.scheduleWithFixedDelay(this::flushBatch, batchWindowMs, batchWindowMs, TimeUnit.MILLISECONDS);
        logger.info("CrossRegionReplicator started: localRegion={}, batchWindow={}ms, maxBatch={}",
                localRegion, batchWindowMs, maxBatchSize);
    }

    /**
     * Enqueues a mutation for cross-region replication.
     *
     * @param mutation the mutation to replicate
     * @param clock    the vector clock at the time of the mutation
     * @throws IllegalStateException if the replicator has been closed
     */
    public void enqueue(ConfigMutation mutation, VectorClock clock) {
        Objects.requireNonNull(mutation, "mutation");
        Objects.requireNonNull(clock, "clock");
        if (closed.get()) {
            throw new IllegalStateException("CrossRegionReplicator is closed, cannot enqueue");
        }
        pendingQueue.add(new ReplicationEntry(mutation, clock));
    }

    /**
     * Returns the total number of successfully replicated mutations.
     */
    public long replicatedCount() {
        return replicatedCount.get();
    }

    /**
     * Returns the total number of failed replication attempts.
     */
    public long failedCount() {
        return failedCount.get();
    }

    /**
     * Returns the number of mutations currently pending replication.
     */
    public int pendingCount() {
        return pendingQueue.size();
    }

    @Override
    public void close() {
        closed.set(true);
        running = false;
        // Final flush
        flushBatch();
        scheduler.shutdown();
        try {
            if (!scheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                scheduler.shutdownNow();
            }
        } catch (InterruptedException e) {
            scheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }
        logger.info("CrossRegionReplicator closed: replicated={}, failed={}", replicatedCount.get(), failedCount.get());
    }

    private void flushBatch() {
        if (pendingQueue.isEmpty()) {
            return;
        }

        // Drain up to maxBatchSize entries
        List<ReplicationEntry> batch = new ArrayList<>(Math.min(maxBatchSize, pendingQueue.size()));
        ReplicationEntry entry;
        while (batch.size() < maxBatchSize && (entry = pendingQueue.poll()) != null) {
            batch.add(entry);
        }

        if (batch.isEmpty()) {
            return;
        }

        long batchId = batchIdGenerator.incrementAndGet();
        ReplicationBatch replicationBatch = new ReplicationBatch(
                localRegion, batch, batchId, Instant.now());

        // Replicate to all healthy remote regions
        List<RegionManager.RegionState> targets = regionManager.healthyRegions().stream()
                .filter(r -> !r.regionId().equals(localRegion))
                .toList();

        if (targets.isEmpty()) {
            // Re-enqueue entries so they are not lost when no remote regions exist
            logger.debug("No remote regions to replicate to, re-enqueuing batch {} with {} entries",
                    batchId, batch.size());
            pendingQueue.addAll(batch);
            return;
        }

        // Count replicated entries once per entry (not once per target per entry).
        // Use a shared flag so we count exactly once when the first target succeeds.
        int entryCount = batch.size();
        AtomicBoolean counted = new AtomicBoolean(false);

        for (RegionManager.RegionState target : targets) {
            replicateWithRetry(target.regionId(), replicationBatch, entryCount, counted, 0);
        }
    }

    /**
     * Attempts to replicate a batch to a target region, retrying on failure up to MAX_RETRIES times.
     */
    private void replicateWithRetry(String targetRegionId, ReplicationBatch batch,
                                    int entryCount, AtomicBoolean counted, int attempt) {
        try {
            callback.replicate(targetRegionId, batch)
                    .whenComplete((v, ex) -> {
                        if (ex != null) {
                            if (attempt < MAX_RETRIES) {
                                logger.warn("Replication to region {} failed for batch {} (attempt {}), retrying: {}",
                                        targetRegionId, batch.batchId(), attempt + 1, ex.getMessage());
                                replicateWithRetry(targetRegionId, batch, entryCount, counted, attempt + 1);
                            } else {
                                failedCount.addAndGet(entryCount);
                                logger.error("Replication to region {} failed for batch {} after {} attempts: {}",
                                        targetRegionId, batch.batchId(), attempt + 1, ex.getMessage());
                            }
                        } else {
                            // Count per-entry once for the batch, not per-target
                            if (counted.compareAndSet(false, true)) {
                                replicatedCount.addAndGet(entryCount);
                            }
                            logger.debug("Replicated batch {} ({} entries) to region {}",
                                    batch.batchId(), entryCount, targetRegionId);
                        }
                    });
        } catch (Exception e) {
            if (attempt < MAX_RETRIES) {
                logger.warn("Failed to submit replication to region {} (attempt {}), retrying",
                        targetRegionId, attempt + 1, e);
                replicateWithRetry(targetRegionId, batch, entryCount, counted, attempt + 1);
            } else {
                failedCount.addAndGet(entryCount);
                logger.error("Failed to submit replication to region {} after {} attempts",
                        targetRegionId, attempt + 1, e);
            }
        }
    }
}
