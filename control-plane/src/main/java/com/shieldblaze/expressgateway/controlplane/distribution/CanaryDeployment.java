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
import lombok.extern.log4j.Log4j2;

import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Manages canary deployments with health-based automatic promotion or rollback.
 *
 * <h3>Workflow</h3>
 * <ol>
 *   <li>Select canary nodes (percentage-based from the target list)</li>
 *   <li>Push the config delta to canary nodes only</li>
 *   <li>Wait for an observation window while monitoring canary health</li>
 *   <li>If canary metrics (error rate, latency) meet the criteria, promote to full fleet</li>
 *   <li>If canary metrics fail, automatically roll back canary nodes</li>
 * </ol>
 *
 * <p>The health evaluator is pluggable via {@link CanaryHealthEvaluator}. The default
 * implementation checks error rate thresholds.</p>
 *
 * <p>Thread safety: this class is safe for concurrent use. The deployment state
 * is tracked via an {@link AtomicReference} with CAS transitions.</p>
 */
@Log4j2
public final class CanaryDeployment {

    /**
     * Deployment state.
     */
    public enum DeploymentState {
        NOT_STARTED,
        CANARY_IN_PROGRESS,
        CANARY_SUCCEEDED,
        CANARY_FAILED,
        PROMOTED,
        ROLLED_BACK
    }

    /**
     * Criteria for evaluating canary health.
     *
     * @param maxErrorRate    the maximum acceptable error rate (0.0 to 1.0)
     * @param maxLatencyMs    the maximum acceptable p99 latency in milliseconds
     * @param observationTime how long to observe canary nodes before making a decision
     */
    public record CanaryCriteria(
            double maxErrorRate,
            long maxLatencyMs,
            Duration observationTime
    ) {
        public CanaryCriteria {
            if (maxErrorRate < 0.0 || maxErrorRate > 1.0) {
                throw new IllegalArgumentException("maxErrorRate must be in [0.0, 1.0]");
            }
            if (maxLatencyMs <= 0) {
                throw new IllegalArgumentException("maxLatencyMs must be > 0");
            }
            Objects.requireNonNull(observationTime, "observationTime");
        }
    }

    /**
     * Evaluates whether canary nodes are healthy enough for promotion.
     */
    @FunctionalInterface
    public interface CanaryHealthEvaluator {
        /**
         * Evaluates the health of canary nodes.
         *
         * @param canaryNodes the nodes running the canary config
         * @param criteria    the health criteria to check against
         * @return true if the canary is healthy enough for promotion
         */
        boolean evaluate(List<DataPlaneNode> canaryNodes, CanaryCriteria criteria);
    }

    /**
     * Callback for rolling back a config delta on a node.
     * The default rollback implementation pushes a reverse delta.
     */
    @FunctionalInterface
    public interface RollbackCallback {
        /**
         * Rolls back a config delta on the given node.
         * @param node the node to roll back
         * @param delta the original delta that was applied
         * @return true if rollback was acknowledged
         */
        boolean rollback(DataPlaneNode node, ConfigDelta delta);
    }

    /**
     * Result of a canary deployment.
     *
     * @param state      the final deployment state
     * @param canaryNodes the nodes that received the canary config
     * @param startTime  when the deployment started
     * @param endTime    when the deployment concluded
     * @param message    a human-readable description
     */
    public record DeploymentResult(
            DeploymentState state,
            List<String> canaryNodes,
            Instant startTime,
            Instant endTime,
            String message
    ) {
        public DeploymentResult {
            Objects.requireNonNull(state, "state");
            Objects.requireNonNull(canaryNodes, "canaryNodes");
            Objects.requireNonNull(startTime, "startTime");
            Objects.requireNonNull(endTime, "endTime");
            Objects.requireNonNull(message, "message");
            canaryNodes = List.copyOf(canaryNodes);
        }
    }

    private final DirectFanOut.ConfigPushCallback pushCallback;
    private final CanaryHealthEvaluator healthEvaluator;
    private final RollbackCallback rollbackCallback;
    private final double canaryPercentage;
    private final int minCanaryNodes;
    private final CanaryCriteria criteria;
    private final AtomicReference<DeploymentState> state = new AtomicReference<>(DeploymentState.NOT_STARTED);
    private final ScheduledExecutorService scheduler;

    /**
     * Creates a new canary deployment manager.
     *
     * @param pushCallback     the callback for pushing config to nodes
     * @param healthEvaluator  evaluator for canary health
     * @param canaryPercentage the percentage of nodes for the canary phase (0.0 to 1.0)
     * @param minCanaryNodes   the minimum number of canary nodes
     * @param criteria         the health criteria for promotion
     */
    public CanaryDeployment(
            DirectFanOut.ConfigPushCallback pushCallback,
            CanaryHealthEvaluator healthEvaluator,
            double canaryPercentage,
            int minCanaryNodes,
            CanaryCriteria criteria) {
        this(pushCallback, healthEvaluator, null, canaryPercentage, minCanaryNodes, criteria);
    }

    /**
     * Creates a new canary deployment manager with an explicit rollback callback.
     *
     * @param pushCallback     the callback for pushing config to nodes
     * @param healthEvaluator  evaluator for canary health
     * @param rollbackCallback the callback for rolling back config on nodes (nullable, uses push as fallback)
     * @param canaryPercentage the percentage of nodes for the canary phase (0.0 to 1.0)
     * @param minCanaryNodes   the minimum number of canary nodes
     * @param criteria         the health criteria for promotion
     */
    public CanaryDeployment(
            DirectFanOut.ConfigPushCallback pushCallback,
            CanaryHealthEvaluator healthEvaluator,
            RollbackCallback rollbackCallback,
            double canaryPercentage,
            int minCanaryNodes,
            CanaryCriteria criteria) {
        this.pushCallback = Objects.requireNonNull(pushCallback, "pushCallback");
        this.healthEvaluator = Objects.requireNonNull(healthEvaluator, "healthEvaluator");
        this.rollbackCallback = rollbackCallback;
        if (canaryPercentage < 0.0 || canaryPercentage > 1.0) {
            throw new IllegalArgumentException("canaryPercentage must be in [0.0, 1.0]");
        }
        this.canaryPercentage = canaryPercentage;
        this.minCanaryNodes = Math.max(1, minCanaryNodes);
        this.criteria = Objects.requireNonNull(criteria, "criteria");
        this.scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "canary-observation");
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Returns the current deployment state.
     */
    public DeploymentState currentState() {
        return state.get();
    }

    /**
     * Executes a canary deployment: pushes to canary nodes, observes health,
     * and either promotes to the full fleet or rolls back.
     *
     * <p>Uses CAS for state transitions to prevent concurrent execution.
     * Uses ScheduledExecutorService for the observation period instead of Thread.sleep.</p>
     *
     * @param delta   the config delta to deploy
     * @param targets all target nodes
     * @return the deployment result
     */
    public DeploymentResult execute(ConfigDelta delta, List<DataPlaneNode> targets) {
        Objects.requireNonNull(delta, "delta");
        Objects.requireNonNull(targets, "targets");

        // CAS: only allow execution from NOT_STARTED state
        if (!state.compareAndSet(DeploymentState.NOT_STARTED, DeploymentState.CANARY_IN_PROGRESS)) {
            DeploymentState current = state.get();
            return new DeploymentResult(current, List.of(), Instant.now(), Instant.now(),
                    "Deployment already in state " + current + ", concurrent execution rejected");
        }

        Instant startTime = Instant.now();

        if (targets.isEmpty()) {
            state.set(DeploymentState.PROMOTED);
            return new DeploymentResult(DeploymentState.PROMOTED, List.of(), startTime, Instant.now(),
                    "No target nodes");
        }

        // Select canary nodes
        List<DataPlaneNode> shuffled = new ArrayList<>(targets);
        Collections.shuffle(shuffled);
        int canarySize = Math.max(minCanaryNodes, (int) Math.ceil(shuffled.size() * canaryPercentage));
        canarySize = Math.min(canarySize, shuffled.size());

        List<DataPlaneNode> canaryNodes = shuffled.subList(0, canarySize);
        List<DataPlaneNode> remainingNodes = shuffled.subList(canarySize, shuffled.size());
        List<String> canaryIds = canaryNodes.stream().map(DataPlaneNode::nodeId).toList();

        log.info("Starting canary deployment: {}/{} nodes, delta [{} -> {}]",
                canarySize, targets.size(), delta.fromRevision(), delta.toRevision());

        // Push to canary nodes, tracking which nodes succeeded
        List<DataPlaneNode> successfulCanaryNodes = new ArrayList<>();
        boolean canaryPushSuccess = true;
        for (DataPlaneNode node : canaryNodes) {
            try {
                boolean accepted = pushCallback.push(node, delta);
                if (!accepted) {
                    canaryPushSuccess = false;
                    log.warn("Canary node {} NACKed the delta", node.nodeId());
                    break;
                }
                successfulCanaryNodes.add(node);
            } catch (Exception e) {
                canaryPushSuccess = false;
                log.error("Failed to push canary config to node {}", node.nodeId(), e);
                break;
            }
        }

        if (!canaryPushSuccess) {
            state.set(DeploymentState.CANARY_FAILED);
            // Rollback all canary nodes that received the new config
            rollbackNodes(successfulCanaryNodes, delta);
            return new DeploymentResult(DeploymentState.CANARY_FAILED, canaryIds, startTime, Instant.now(),
                    "Canary push failed: one or more canary nodes rejected the config");
        }

        // Observe canary health using ScheduledExecutorService (no Thread.sleep)
        log.info("Canary push succeeded, observing for {}", criteria.observationTime());
        CompletableFuture<Void> observationFuture = new CompletableFuture<>();
        scheduler.schedule(() -> observationFuture.complete(null),
                criteria.observationTime().toMillis(), TimeUnit.MILLISECONDS);

        try {
            observationFuture.get();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            state.set(DeploymentState.ROLLED_BACK);
            rollbackNodes(successfulCanaryNodes, delta);
            return new DeploymentResult(DeploymentState.ROLLED_BACK, canaryIds, startTime, Instant.now(),
                    "Canary observation interrupted");
        } catch (Exception e) {
            state.set(DeploymentState.ROLLED_BACK);
            rollbackNodes(successfulCanaryNodes, delta);
            return new DeploymentResult(DeploymentState.ROLLED_BACK, canaryIds, startTime, Instant.now(),
                    "Canary observation failed: " + e.getMessage());
        }

        // Evaluate canary health
        boolean healthy = healthEvaluator.evaluate(canaryNodes, criteria);

        if (!healthy) {
            state.set(DeploymentState.ROLLED_BACK);
            log.warn("Canary health check failed, rolling back canary nodes");
            rollbackNodes(successfulCanaryNodes, delta);
            return new DeploymentResult(DeploymentState.ROLLED_BACK, canaryIds, startTime, Instant.now(),
                    "Canary health check failed: error rate or latency exceeded thresholds");
        }

        // Canary passed -- promote to remaining nodes
        state.set(DeploymentState.CANARY_SUCCEEDED);
        log.info("Canary health check passed, promoting to remaining {} nodes", remainingNodes.size());

        List<String> failedPromotionNodes = new ArrayList<>();
        for (DataPlaneNode node : remainingNodes) {
            try {
                boolean accepted = pushCallback.push(node, delta);
                if (!accepted) {
                    failedPromotionNodes.add(node.nodeId());
                    log.warn("Node {} NACKed during promotion", node.nodeId());
                }
            } catch (Exception e) {
                failedPromotionNodes.add(node.nodeId());
                log.error("Failed to push config to node {} during promotion", node.nodeId(), e);
            }
        }

        if (!failedPromotionNodes.isEmpty()) {
            log.warn("Promotion completed with {} failures: {}", failedPromotionNodes.size(), failedPromotionNodes);
        }

        state.set(DeploymentState.PROMOTED);
        return new DeploymentResult(DeploymentState.PROMOTED, canaryIds, startTime, Instant.now(),
                "Canary deployment succeeded: promoted to all " + targets.size() + " nodes" +
                        (failedPromotionNodes.isEmpty() ? "" : " (" + failedPromotionNodes.size() + " failed)"));
    }

    /**
     * Sends rollback to all nodes that received the canary config.
     */
    private void rollbackNodes(List<DataPlaneNode> nodes, ConfigDelta delta) {
        for (DataPlaneNode node : nodes) {
            try {
                if (rollbackCallback != null) {
                    rollbackCallback.rollback(node, delta);
                } else {
                    // Fallback: push a reverse delta (from toRevision back to fromRevision)
                    ConfigDelta reverseDelta = new ConfigDelta(delta.toRevision(), delta.fromRevision(), List.of());
                    pushCallback.push(node, reverseDelta);
                }
                log.info("Rolled back canary config on node {}", node.nodeId());
            } catch (Exception e) {
                log.error("Failed to rollback canary config on node {}", node.nodeId(), e);
            }
        }
    }
}
