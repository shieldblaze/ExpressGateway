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

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;

/**
 * GAP-DIST-1: Canary fan-out strategy that rolls out config changes in two phases:
 * <ol>
 *   <li><b>Canary phase:</b> Push to a configurable percentage of target nodes first.</li>
 *   <li><b>Full rollout phase:</b> If all canary nodes accept the delta, push to the remaining nodes.</li>
 * </ol>
 *
 * <p>If any canary node NACKs or fails, the rollout is aborted and the remaining nodes
 * are NOT updated. This prevents a bad config from being pushed to the entire fleet.</p>
 *
 * <p>The canary set is selected randomly by shuffling the target list. For fleets with
 * region-aware node assignment, the shuffle provides natural distribution across regions.</p>
 *
 * <p>Delegates actual per-node push operations to a {@link DirectFanOut.ConfigPushCallback},
 * keeping the canary logic independent of transport.</p>
 */
@Log4j2
public final class CanaryFanOut implements FanOutStrategy {

    private final DirectFanOut.ConfigPushCallback pushCallback;
    private final DistributionMetrics metrics;
    private final double canaryPercentage;
    private final int minCanaryNodes;

    /**
     * @param pushCallback     the transport-level push function; must not be null
     * @param metrics          optional distribution metrics; may be null to disable metrics
     * @param canaryPercentage percentage of nodes to include in the canary phase (0.0-1.0, e.g., 0.1 for 10%)
     * @param minCanaryNodes   minimum number of canary nodes (at least 1); if the fleet is smaller, all nodes are canary
     */
    public CanaryFanOut(DirectFanOut.ConfigPushCallback pushCallback,
                        DistributionMetrics metrics,
                        double canaryPercentage,
                        int minCanaryNodes) {
        this.pushCallback = Objects.requireNonNull(pushCallback, "pushCallback");
        this.metrics = metrics;
        if (canaryPercentage < 0.0 || canaryPercentage > 1.0) {
            throw new IllegalArgumentException("canaryPercentage must be between 0.0 and 1.0, got: " + canaryPercentage);
        }
        this.canaryPercentage = canaryPercentage;
        this.minCanaryNodes = Math.max(1, minCanaryNodes);
    }

    /**
     * Convenience constructor with default 10% canary and minimum 1 node.
     */
    public CanaryFanOut(DirectFanOut.ConfigPushCallback pushCallback, DistributionMetrics metrics) {
        this(pushCallback, metrics, 0.10, 1);
    }

    @Override
    public void distribute(ConfigDelta delta, List<DataPlaneNode> targets) {
        Objects.requireNonNull(delta, "delta");
        Objects.requireNonNull(targets, "targets");

        if (targets.isEmpty()) {
            return;
        }

        // Shuffle to randomize canary selection
        List<DataPlaneNode> shuffled = new ArrayList<>(targets);
        Collections.shuffle(shuffled);

        // Compute canary set size
        int canarySize = Math.max(minCanaryNodes, (int) Math.ceil(shuffled.size() * canaryPercentage));
        canarySize = Math.min(canarySize, shuffled.size()); // Don't exceed fleet size

        List<DataPlaneNode> canaryNodes = shuffled.subList(0, canarySize);
        List<DataPlaneNode> remainingNodes = shuffled.subList(canarySize, shuffled.size());

        log.info("Canary rollout: pushing delta [{} -> {}] to {}/{} canary nodes first",
                delta.fromRevision(), delta.toRevision(), canarySize, shuffled.size());

        // Phase 1: Canary push
        boolean canarySuccess = pushToNodes(delta, canaryNodes);

        if (!canarySuccess) {
            log.warn("Canary rollout ABORTED: one or more canary nodes rejected delta [{} -> {}]. " +
                            "Remaining {} nodes will NOT be updated.",
                    delta.fromRevision(), delta.toRevision(), remainingNodes.size());
            return;
        }

        // Phase 2: Full rollout
        if (!remainingNodes.isEmpty()) {
            log.info("Canary phase succeeded. Rolling out to remaining {} nodes.", remainingNodes.size());
            pushToNodes(delta, remainingNodes);
        }
    }

    /**
     * Push delta to a list of nodes. Returns {@code true} if ALL nodes accepted.
     */
    private boolean pushToNodes(ConfigDelta delta, List<DataPlaneNode> nodes) {
        boolean allAccepted = true;
        for (DataPlaneNode node : nodes) {
            io.micrometer.core.instrument.Timer.Sample sample =
                    metrics != null ? metrics.startTimer() : null;
            try {
                boolean accepted = pushCallback.push(node, delta);
                if (sample != null) {
                    metrics.stopTimer(sample);
                }
                if (accepted) {
                    if (metrics != null) {
                        metrics.recordSuccess();
                    }
                } else {
                    log.warn("Node {} NACKed config delta [{} -> {}]",
                            node.nodeId(), delta.fromRevision(), delta.toRevision());
                    if (metrics != null) {
                        metrics.recordNack();
                    }
                    allAccepted = false;
                }
            } catch (Exception e) {
                if (sample != null) {
                    metrics.stopTimer(sample);
                }
                log.error("Failed to push config to node {}", node.nodeId(), e);
                if (metrics != null) {
                    metrics.recordFailure();
                }
                allAccepted = false;
            }
        }
        return allAccepted;
    }
}
