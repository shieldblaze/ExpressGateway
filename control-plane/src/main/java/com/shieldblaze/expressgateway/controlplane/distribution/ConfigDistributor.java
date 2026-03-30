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

import com.shieldblaze.expressgateway.controlplane.config.ChangeJournal;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigTransaction;
import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

/**
 * Orchestrates config distribution: receives mutations, batches them,
 * journals them, computes deltas, and pushes to connected nodes.
 *
 * <p>Single-writer pattern: all mutations flow through
 * {@link WriteBatcher} -> flush -> {@link ChangeJournal#append} -> {@link FanOutStrategy#distribute}.
 * This eliminates locks on config state because the batcher's flush callback
 * runs on a single dedicated thread.</p>
 *
 * <p>Fan-out is performed asynchronously on a dedicated thread pool
 * ({@code fanOutExecutor}) so that the WriteBatcher's flush callback is not
 * blocked by slow or unreachable nodes. Nodes are grouped by their
 * {@code appliedConfigVersion} so that each distinct delta is computed only once,
 * even if many nodes are at the same version.</p>
 *
 * <p>Lifecycle:</p>
 * <ol>
 *   <li>Construct with all dependencies</li>
 *   <li>Call {@link #start()} to begin the batching scheduler</li>
 *   <li>Submit mutations via {@link #submit(ConfigTransaction)} or {@link #submit(ConfigMutation)}</li>
 *   <li>Call {@link #close()} to drain remaining mutations and stop the scheduler</li>
 * </ol>
 */
@Log4j2
public final class ConfigDistributor implements Closeable {

    private final ChangeJournal journal;
    private final DeltaSyncEngine deltaEngine;
    private final WriteBatcher batcher;
    private final FanOutStrategy fanOut;
    private final NodeRegistry nodeRegistry;
    private final ExecutorService fanOutExecutor;
    private final DistributionMetrics metrics;

    /**
     * @param journal        the change journal for persisting mutations; must not be null
     * @param nodeRegistry   the registry of connected data-plane nodes; must not be null
     * @param fanOut         the distribution strategy; must not be null
     * @param batchWindowMs  the batching window in milliseconds (e.g., 500)
     * @param maxJournalLag  maximum journal entries a node can lag before requiring full snapshot
     */
    public ConfigDistributor(
            ChangeJournal journal,
            NodeRegistry nodeRegistry,
            FanOutStrategy fanOut,
            long batchWindowMs,
            long maxJournalLag) {
        this(journal, nodeRegistry, fanOut, batchWindowMs, maxJournalLag, null);
    }

    /**
     * @param journal        the change journal for persisting mutations; must not be null
     * @param nodeRegistry   the registry of connected data-plane nodes; must not be null
     * @param fanOut         the distribution strategy; must not be null
     * @param batchWindowMs  the batching window in milliseconds (e.g., 500)
     * @param maxJournalLag  maximum journal entries a node can lag before requiring full snapshot
     * @param metrics        optional distribution metrics; may be null to disable metrics
     */
    public ConfigDistributor(
            ChangeJournal journal,
            NodeRegistry nodeRegistry,
            FanOutStrategy fanOut,
            long batchWindowMs,
            long maxJournalLag,
            DistributionMetrics metrics) {
        this.journal = Objects.requireNonNull(journal, "journal");
        this.nodeRegistry = Objects.requireNonNull(nodeRegistry, "nodeRegistry");
        this.fanOut = Objects.requireNonNull(fanOut, "fanOut");
        this.deltaEngine = new DeltaSyncEngine(journal, maxJournalLag);
        this.batcher = new WriteBatcher(batchWindowMs, this::onBatchFlush);
        this.fanOutExecutor = Executors.newThreadPerTaskExecutor(
                Thread.ofVirtual().name("cp-fan-out-", 0).factory());
        this.metrics = metrics;
    }

    /**
     * Start the batching scheduler. Must be called once before submitting mutations.
     */
    public void start() {
        batcher.start();
    }

    /**
     * Submit a config transaction for distribution.
     * All mutations in the transaction will be batched together.
     *
     * @param transaction the transaction to distribute; must not be null
     * @return a future that completes when all mutations in the transaction are committed
     */
    public CompletableFuture<Void> submit(ConfigTransaction transaction) {
        Objects.requireNonNull(transaction, "transaction");
        return batcher.submitAll(transaction.mutations());
    }

    /**
     * Submit a single mutation for distribution.
     *
     * @param mutation the mutation to distribute; must not be null
     * @return a future that completes when the mutation is committed
     */
    public CompletableFuture<Void> submit(ConfigMutation mutation) {
        Objects.requireNonNull(mutation, "mutation");
        return batcher.submit(mutation);
    }

    /**
     * Callback invoked by the {@link WriteBatcher} on each flush cycle.
     * Runs on the batcher's single scheduler thread.
     *
     * <p>Journal failures are allowed to propagate so that {@link WriteBatcher}
     * re-queues the batch for retry. Fan-out is dispatched asynchronously to
     * {@code fanOutExecutor} so the batcher thread is never blocked by slow nodes.</p>
     *
     * <p>Nodes are grouped by {@code appliedConfigVersion} so that each distinct
     * delta is computed only once. This avoids redundant delta computation when
     * many nodes are at the same version (common after a bulk update or restart).</p>
     *
     * @throws Exception if journaling fails (triggers WriteBatcher retry)
     */
    private void onBatchFlush(List<ConfigMutation> batch) throws Exception {
        // 1. Journal the mutations -- this assigns a new global revision.
        //    Let exceptions propagate so WriteBatcher re-queues the batch.
        long newRevision = journal.append(batch);
        log.info("Journaled {} mutations at revision {}", batch.size(), newRevision);

        // Record batch size metric
        if (metrics != null) {
            metrics.recordBatchSize(batch.size());
        }

        // 2. Distribute deltas to all healthy nodes asynchronously.
        //    Fan-out errors are non-fatal: the journal has the data and nodes
        //    will catch up on the next push cycle or on reconnect.
        fanOutExecutor.submit(() -> {
            try {
                List<DataPlaneNode> targets = nodeRegistry.healthyNodes();
                if (targets.isEmpty()) {
                    log.debug("No healthy nodes to distribute to");
                    return;
                }

                // Group nodes by appliedConfigVersion to compute each delta only once.
                Map<Long, List<DataPlaneNode>> nodesByVersion = new HashMap<>();
                for (DataPlaneNode node : targets) {
                    nodesByVersion
                            .computeIfAbsent(node.appliedConfigVersion(), k -> new ArrayList<>())
                            .add(node);
                }

                for (Map.Entry<Long, List<DataPlaneNode>> entry : nodesByVersion.entrySet()) {
                    long version = entry.getKey();
                    List<DataPlaneNode> nodesAtVersion = entry.getValue();

                    ConfigDelta delta = deltaEngine.computeDelta(version);
                    if (delta == null || delta.isEmpty()) {
                        continue;
                    }
                    fanOut.distribute(delta, nodesAtVersion);
                }
            } catch (Exception e) {
                log.error("Fan-out failed for revision {}, nodes will catch up on next push", newRevision, e);
            }

            // Compact journal: remove entries that all healthy nodes have already ACKed
            try {
                List<DataPlaneNode> allNodes = nodeRegistry.healthyNodes();
                if (!allNodes.isEmpty()) {
                    long minAcked = allNodes.stream()
                            .mapToLong(DataPlaneNode::appliedConfigVersion)
                            .min()
                            .orElse(0);
                    if (minAcked > 0) {
                        journal.compact(minAcked);
                    }
                }
            } catch (Exception e) {
                log.debug("Journal compaction skipped: {}", e.getMessage());
            }
        });
    }

    /**
     * Compute the delta for a specific node, e.g., on reconnect or force-sync.
     *
     * @param nodeLastRevision the node's last acknowledged revision
     * @return the delta, or {@code null} if a full snapshot is required
     */
    public ConfigDelta computeNodeDelta(long nodeLastRevision) {
        return deltaEngine.computeDelta(nodeLastRevision);
    }

    /**
     * Returns the current (latest) global config revision from the underlying journal.
     *
     * @return the current revision, or 0 if no entries exist
     */
    public long currentRevision() {
        return journal.currentRevision();
    }

    @Override
    public void close() {
        batcher.close();
        fanOutExecutor.shutdown();
        try {
            if (!fanOutExecutor.awaitTermination(10, TimeUnit.SECONDS)) {
                log.warn("Fan-out executor did not terminate within 10 seconds, forcing shutdown");
                fanOutExecutor.shutdownNow();
            }
        } catch (InterruptedException e) {
            fanOutExecutor.shutdownNow();
            Thread.currentThread().interrupt();
        }
    }
}
