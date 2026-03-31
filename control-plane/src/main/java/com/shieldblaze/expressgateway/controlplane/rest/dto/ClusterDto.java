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
package com.shieldblaze.expressgateway.controlplane.rest.dto;

import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;

import java.util.Map;

/**
 * DTO for cluster configuration CRUD operations.
 *
 * @param name                the cluster name
 * @param loadBalanceStrategy the LB strategy (e.g. "round-robin", "least-connection")
 * @param healthCheckName     reference to a health check by name
 * @param maxConnections      maximum total connections to this cluster
 * @param drainTimeoutSeconds drain timeout in seconds for graceful node removal
 * @param labels              arbitrary key-value labels for filtering
 */
public record ClusterDto(
        String name,
        String loadBalanceStrategy,
        String healthCheckName,
        int maxConnections,
        int drainTimeoutSeconds,
        Map<String, String> labels
) {

    /**
     * Convert this DTO to a {@link ClusterSpec}.
     */
    public ClusterSpec toSpec() {
        return new ClusterSpec(name, loadBalanceStrategy, healthCheckName, maxConnections, drainTimeoutSeconds);
    }

    /**
     * Create a DTO from a {@link ClusterSpec} and labels.
     */
    public static ClusterDto from(ClusterSpec spec, Map<String, String> labels) {
        return new ClusterDto(
                spec.name(),
                spec.loadBalanceStrategy(),
                spec.healthCheckName(),
                spec.maxConnections(),
                spec.drainTimeoutSeconds(),
                labels
        );
    }
}
