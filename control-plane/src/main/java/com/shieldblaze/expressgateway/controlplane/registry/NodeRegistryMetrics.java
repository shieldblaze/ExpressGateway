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
package com.shieldblaze.expressgateway.controlplane.registry;

import io.micrometer.core.instrument.Gauge;
import io.micrometer.core.instrument.MeterRegistry;

import java.util.Objects;

/**
 * Registers Micrometer gauges that expose the current state distribution of data-plane
 * nodes in the {@link NodeRegistry}.
 *
 * <p>Registered gauges:
 * <ul>
 *   <li>{@code expressgateway.controlplane.nodes} with tag {@code state=connected|healthy|unhealthy|draining|disconnected}</li>
 *   <li>{@code expressgateway.controlplane.nodes.total} -- total registered nodes across all states</li>
 * </ul>
 *
 * <p>Gauges use supplier-based registration, so each scrape invokes a live query against
 * the registry. This is O(N) per state per scrape, which is acceptable for control-plane
 * cardinality (typically &lt; 10K nodes).</p>
 */
public final class NodeRegistryMetrics {

    private final NodeRegistry registry;
    private final MeterRegistry meterRegistry;

    /**
     * Creates and registers all node registry gauges.
     *
     * @param registry      the node registry to monitor; must not be null
     * @param meterRegistry the Micrometer meter registry to register gauges on; must not be null
     */
    public NodeRegistryMetrics(NodeRegistry registry, MeterRegistry meterRegistry) {
        this.registry = Objects.requireNonNull(registry, "registry");
        this.meterRegistry = Objects.requireNonNull(meterRegistry, "meterRegistry");
        registerMetrics();
    }

    private void registerMetrics() {
        for (DataPlaneNodeState state : DataPlaneNodeState.values()) {
            // Capture state in a local variable for the lambda closure.
            DataPlaneNodeState capturedState = state;
            Gauge.builder("expressgateway.controlplane.nodes",
                            () -> registry.nodesByState(capturedState).size())
                    .tag("state", state.name().toLowerCase())
                    .description("Number of data plane nodes in " + state + " state")
                    .register(meterRegistry);
        }

        Gauge.builder("expressgateway.controlplane.nodes.total", registry::size)
                .description("Total number of registered data plane nodes")
                .register(meterRegistry);
    }
}
