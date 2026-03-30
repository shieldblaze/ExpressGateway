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

import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;
import java.time.Instant;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Two-phase atomic config rollout with per-node acknowledgment tracking and rollback.
 *
 * <h3>Protocol</h3>
 * <ol>
 *   <li><b>Prepare phase:</b> Push the config delta to all target nodes. Each node
 *       validates the config but does NOT apply it yet. Nodes that accept respond
 *       with PREPARE_ACK; nodes that reject respond with PREPARE_NACK.</li>
 *   <li><b>Decision:</b> If all nodes PREPARE_ACK, the coordinator sends COMMIT.
 *       If any node PREPARE_NACKs or times out, the coordinator sends ABORT.</li>
 *   <li><b>Commit/Abort phase:</b> On COMMIT, all nodes apply the config. On ABORT,
 *       all nodes discard the prepared config.</li>
 * </ol>
 *
 * <p>Timeout-based failure detection: nodes that do not respond within the
 * configured timeout are treated as failures, triggering an abort.</p>
 *
 * <p>Thread safety: this class is safe for concurrent use. Each rollout creates
 * its own tracking state.</p>
 */
public final class AtomicRollout {

    private static final Logger logger = LogManager.getLogger(AtomicRollout.class);

    /** Maximum number of retry attempts for failed commit ACKs. */
    private static final int COMMIT_MAX_RETRIES = 1;

    /**
     * Phase of the rollout.
     */
    public enum Phase {
        PREPARE,
        COMMITTED,
        ABORTED
    }

    /**
     * Per-node acknowledgment status.
     */
    public enum AckStatus {
        PENDING,
        PREPARE_ACK,
        PREPARE_NACK,
        COMMIT_ACK,
        TIMEOUT
    }

    /**
     * Callback for the two-phase protocol operations.
     */
    public interface RolloutCallback {
        /**
         * Sends a prepare request to the node. Returns true if the node ACKs the prepare.
         */
        boolean prepare(DataPlaneNode node, ConfigDelta delta);

        /**
         * Sends a commit request to the node. Returns true if the node ACKs the commit.
         */
        boolean commit(DataPlaneNode node, ConfigDelta delta);

        /**
         * Sends an abort request to the node. Best-effort; return value is informational.
         */
        boolean abort(DataPlaneNode node, ConfigDelta delta);
    }

    /**
     * Result of an atomic rollout attempt.
     *
     * @param phase      the final phase of the rollout
     * @param nodeStatus per-node acknowledgment status
     * @param startTime  when the rollout started
     * @param endTime    when the rollout completed
     */
    public record RolloutResult(
            Phase phase,
            Map<String, AckStatus> nodeStatus,
            Instant startTime,
            Instant endTime
    ) {
        public RolloutResult {
            Objects.requireNonNull(phase, "phase");
            Objects.requireNonNull(nodeStatus, "nodeStatus");
            Objects.requireNonNull(startTime, "startTime");
            Objects.requireNonNull(endTime, "endTime");
            nodeStatus = Collections.unmodifiableMap(nodeStatus);
        }

        /**
         * Returns true if the rollout was committed successfully.
         */
        public boolean isCommitted() {
            return phase == Phase.COMMITTED;
        }

        /**
         * Returns the duration of the rollout.
         */
        public Duration duration() {
            return Duration.between(startTime, endTime);
        }
    }

    private final RolloutCallback callback;
    private final Duration prepareTimeout;
    private final Duration commitTimeout;

    /**
     * Creates a new atomic rollout coordinator.
     *
     * @param callback       the protocol callback for node communication
     * @param prepareTimeout the maximum time to wait for all prepare responses
     */
    public AtomicRollout(RolloutCallback callback, Duration prepareTimeout) {
        this(callback, prepareTimeout, prepareTimeout);
    }

    /**
     * Creates a new atomic rollout coordinator with separate prepare and commit timeouts.
     *
     * @param callback       the protocol callback for node communication
     * @param prepareTimeout the maximum time to wait for all prepare responses
     * @param commitTimeout  the maximum time to wait for all commit responses
     */
    public AtomicRollout(RolloutCallback callback, Duration prepareTimeout, Duration commitTimeout) {
        this.callback = Objects.requireNonNull(callback, "callback");
        this.prepareTimeout = Objects.requireNonNull(prepareTimeout, "prepareTimeout");
        this.commitTimeout = Objects.requireNonNull(commitTimeout, "commitTimeout");
        if (prepareTimeout.isNegative() || prepareTimeout.isZero()) {
            throw new IllegalArgumentException("prepareTimeout must be positive");
        }
        if (commitTimeout.isNegative() || commitTimeout.isZero()) {
            throw new IllegalArgumentException("commitTimeout must be positive");
        }
    }

    /**
     * Executes a two-phase atomic rollout to the target nodes.
     *
     * <p>This method blocks until the rollout completes (commit or abort) or times out.</p>
     *
     * @param delta   the config delta to roll out
     * @param targets the target nodes
     * @return the rollout result
     */
    public RolloutResult execute(ConfigDelta delta, List<DataPlaneNode> targets) {
        Objects.requireNonNull(delta, "delta");
        Objects.requireNonNull(targets, "targets");

        Instant startTime = Instant.now();
        ConcurrentHashMap<String, AckStatus> nodeStatus = new ConcurrentHashMap<>();

        if (targets.isEmpty()) {
            return new RolloutResult(Phase.ABORTED, Map.of(), startTime, Instant.now());
        }

        // Initialize all nodes as PENDING
        for (DataPlaneNode node : targets) {
            nodeStatus.put(node.nodeId(), AckStatus.PENDING);
        }

        // Phase 1: Prepare -- send prepare to all nodes concurrently using virtual threads
        CountDownLatch prepareLatch = new CountDownLatch(targets.size());
        AtomicReference<Boolean> allPrepared = new AtomicReference<>(true);

        for (DataPlaneNode node : targets) {
            Thread.ofVirtual().name("rollout-prepare-" + node.nodeId()).start(() -> {
                try {
                    boolean accepted = callback.prepare(node, delta);
                    nodeStatus.put(node.nodeId(), accepted ? AckStatus.PREPARE_ACK : AckStatus.PREPARE_NACK);
                    if (!accepted) {
                        allPrepared.set(false);
                    }
                } catch (Exception e) {
                    logger.error("Prepare failed for node {}", node.nodeId(), e);
                    nodeStatus.put(node.nodeId(), AckStatus.PREPARE_NACK);
                    allPrepared.set(false);
                } finally {
                    prepareLatch.countDown();
                }
            });
        }

        // Wait for all prepares with timeout
        try {
            boolean completed = prepareLatch.await(prepareTimeout.toMillis(), TimeUnit.MILLISECONDS);
            if (!completed) {
                // Mark timed-out nodes
                for (Map.Entry<String, AckStatus> entry : nodeStatus.entrySet()) {
                    if (entry.getValue() == AckStatus.PENDING) {
                        entry.setValue(AckStatus.TIMEOUT);
                    }
                }
                allPrepared.set(false);
                logger.warn("Prepare phase timed out after {}", prepareTimeout);
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return new RolloutResult(Phase.ABORTED, new java.util.HashMap<>(nodeStatus), startTime, Instant.now());
        }

        // Phase 2: Commit or Abort
        if (allPrepared.get()) {
            // All nodes prepared -- commit
            logger.info("All {} nodes prepared, sending COMMIT for delta [{} -> {}]",
                    targets.size(), delta.fromRevision(), delta.toRevision());
            boolean commitSuccess = commitAll(delta, targets, nodeStatus);
            Phase phase = commitSuccess ? Phase.COMMITTED : Phase.ABORTED;
            return new RolloutResult(phase, new java.util.HashMap<>(nodeStatus), startTime, Instant.now());
        } else {
            // At least one node failed -- abort all
            Set<String> failedNodes = nodeStatus.entrySet().stream()
                    .filter(e -> e.getValue() != AckStatus.PREPARE_ACK)
                    .map(Map.Entry::getKey)
                    .collect(java.util.stream.Collectors.toSet());
            logger.warn("Prepare phase failed for nodes {}, sending ABORT for delta [{} -> {}]",
                    failedNodes, delta.fromRevision(), delta.toRevision());
            abortAll(delta, targets, nodeStatus);
            return new RolloutResult(Phase.ABORTED, new java.util.HashMap<>(nodeStatus), startTime, Instant.now());
        }
    }

    /**
     * Commits all nodes with retry logic. Returns true only if ALL nodes ACK the commit.
     */
    private boolean commitAll(ConfigDelta delta, List<DataPlaneNode> targets,
                              ConcurrentHashMap<String, AckStatus> nodeStatus) {
        CountDownLatch commitLatch = new CountDownLatch(targets.size());
        for (DataPlaneNode node : targets) {
            Thread.ofVirtual().name("rollout-commit-" + node.nodeId()).start(() -> {
                try {
                    boolean ack = commitWithRetry(node, delta);
                    nodeStatus.put(node.nodeId(), ack ? AckStatus.COMMIT_ACK : AckStatus.PREPARE_NACK);
                } catch (Exception e) {
                    logger.error("Commit failed for node {} after retries", node.nodeId(), e);
                    nodeStatus.put(node.nodeId(), AckStatus.PREPARE_NACK);
                } finally {
                    commitLatch.countDown();
                }
            });
        }
        try {
            commitLatch.await(commitTimeout.toMillis(), TimeUnit.MILLISECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }

        // Check if ALL nodes committed successfully
        for (DataPlaneNode node : targets) {
            AckStatus status = nodeStatus.get(node.nodeId());
            if (status != AckStatus.COMMIT_ACK) {
                logger.warn("Commit phase failed: node {} has status {}", node.nodeId(), status);
                return false;
            }
        }
        return true;
    }

    /**
     * Attempts to commit a single node, retrying up to {@link #COMMIT_MAX_RETRIES} times on failure.
     */
    private boolean commitWithRetry(DataPlaneNode node, ConfigDelta delta) {
        boolean ack = false;
        for (int attempt = 0; attempt <= COMMIT_MAX_RETRIES; attempt++) {
            try {
                ack = callback.commit(node, delta);
                if (ack) {
                    return true;
                }
            } catch (Exception e) {
                logger.warn("Commit attempt {} failed for node {}", attempt + 1, node.nodeId(), e);
            }
            if (attempt < COMMIT_MAX_RETRIES) {
                logger.info("Retrying commit for node {} (attempt {})", node.nodeId(), attempt + 2);
            }
        }
        return ack;
    }

    /**
     * Sends abort to all nodes and waits for completion (not fire-and-forget).
     */
    private void abortAll(ConfigDelta delta, List<DataPlaneNode> targets,
                          ConcurrentHashMap<String, AckStatus> nodeStatus) {
        CountDownLatch abortLatch = new CountDownLatch(targets.size());
        for (DataPlaneNode node : targets) {
            Thread.ofVirtual().name("rollout-abort-" + node.nodeId()).start(() -> {
                try {
                    callback.abort(node, delta);
                } catch (Exception e) {
                    logger.debug("Abort notification failed for node {} (best-effort)", node.nodeId(), e);
                } finally {
                    abortLatch.countDown();
                }
            });
        }
        try {
            abortLatch.await(prepareTimeout.toMillis(), TimeUnit.MILLISECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }
}
