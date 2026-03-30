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

import java.util.List;
import java.util.Objects;

/**
 * Direct fan-out: the control plane pushes config deltas to all target nodes directly.
 *
 * <p>Simple and correct for fleets up to ~10K nodes. The actual gRPC push mechanism
 * is injected via {@link ConfigPushCallback}, keeping this class protocol-agnostic
 * and testable without a running gRPC server.</p>
 *
 * <p>Failure semantics: individual node push failures are logged but do not abort
 * distribution to the remaining targets. Nodes that NACK or fail will still have
 * a stale {@code appliedConfigVersion} and will receive the correct delta on the
 * next push cycle.</p>
 */
@Log4j2
public final class DirectFanOut implements FanOutStrategy {

    /**
     * Callback for pushing a config delta to a single data-plane node.
     *
     * <p>Implementations should perform the actual transport-level push (e.g., gRPC
     * {@code ConfigDistributionService.PushConfig}) and return the node's response:</p>
     * <ul>
     *   <li>{@code true} -- node ACKed the delta</li>
     *   <li>{@code false} -- node NACKed the delta (e.g., validation failure, version conflict)</li>
     * </ul>
     *
     * <p>Throwing an exception signals a transport-level failure (node unreachable,
     * timeout, stream reset, etc.).</p>
     */
    @FunctionalInterface
    public interface ConfigPushCallback {
        boolean push(DataPlaneNode node, ConfigDelta delta);
    }

    private final ConfigPushCallback pushCallback;
    private final DistributionMetrics metrics;

    /**
     * @param pushCallback the transport-level push function; must not be null
     */
    public DirectFanOut(ConfigPushCallback pushCallback) {
        this(pushCallback, null);
    }

    /**
     * @param pushCallback the transport-level push function; must not be null
     * @param metrics      optional distribution metrics; may be null to disable metrics
     */
    public DirectFanOut(ConfigPushCallback pushCallback, DistributionMetrics metrics) {
        this.pushCallback = Objects.requireNonNull(pushCallback, "pushCallback");
        this.metrics = metrics;
    }

    @Override
    public void distribute(ConfigDelta delta, List<DataPlaneNode> targets) {
        Objects.requireNonNull(delta, "delta");
        Objects.requireNonNull(targets, "targets");

        for (DataPlaneNode node : targets) {
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
                }
            } catch (Exception e) {
                if (sample != null) {
                    metrics.stopTimer(sample);
                }
                log.error("Failed to push config to node {}", node.nodeId(), e);
                if (metrics != null) {
                    metrics.recordFailure();
                }
            }
        }
    }
}
