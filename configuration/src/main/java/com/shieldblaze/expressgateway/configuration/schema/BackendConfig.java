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
package com.shieldblaze.expressgateway.configuration.schema;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;
import java.util.Objects;
import java.util.Set;

/**
 * Configuration schema for a backend cluster.
 *
 * @param name                  The cluster name
 * @param nodes                 The list of backend node addresses (host:port)
 * @param loadBalanceStrategy   The load balancing strategy
 * @param healthCheckRef        Optional reference to a health check config by name
 * @param circuitBreakerRef     Optional reference to a {@link CircuitBreakerConfig} by name
 * @param maxConnectionsPerNode Maximum connections per backend node (must be >= 1)
 * @param connectionPoolSize    Connection pool size per backend node (must be >= 0; 0 means no pooling)
 */
public record BackendConfig(
        @JsonProperty("name") String name,
        @JsonProperty("nodes") List<String> nodes,
        @JsonProperty("loadBalanceStrategy") String loadBalanceStrategy,
        @JsonProperty("healthCheckRef") String healthCheckRef,
        @JsonProperty("circuitBreakerRef") String circuitBreakerRef,
        @JsonProperty("maxConnectionsPerNode") int maxConnectionsPerNode,
        @JsonProperty("connectionPoolSize") int connectionPoolSize
) {

    private static final Set<String> VALID_STRATEGIES = Set.of(
            "round-robin", "least-connection", "random", "ip-hash", "weighted-round-robin"
    );

    public BackendConfig {
        // Defensive copy of nodes list
        nodes = nodes == null ? List.of() : List.copyOf(nodes);
    }

    /**
     * Validate all fields for correctness.
     *
     * @throws IllegalArgumentException if any field is invalid
     */
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        if (nodes.isEmpty()) {
            throw new IllegalArgumentException("nodes must not be empty");
        }
        for (int i = 0; i < nodes.size(); i++) {
            String node = nodes.get(i);
            if (node == null || node.isBlank()) {
                throw new IllegalArgumentException("nodes[" + i + "] must not be null or blank");
            }
            // Validate host:port format
            int lastColon = node.lastIndexOf(':');
            if (lastColon <= 0 || lastColon == node.length() - 1) {
                throw new IllegalArgumentException("nodes[" + i + "] must be in host:port format, got: " + node);
            }
            String portStr = node.substring(lastColon + 1);
            try {
                int port = Integer.parseInt(portStr);
                if (port < 1 || port > 65535) {
                    throw new IllegalArgumentException("nodes[" + i + "] port must be in range [1, 65535], got: " + port);
                }
            } catch (NumberFormatException e) {
                throw new IllegalArgumentException("nodes[" + i + "] has invalid port: " + portStr, e);
            }
        }
        Objects.requireNonNull(loadBalanceStrategy, "loadBalanceStrategy");
        if (!VALID_STRATEGIES.contains(loadBalanceStrategy)) {
            throw new IllegalArgumentException(
                    "loadBalanceStrategy must be one of " + VALID_STRATEGIES + ", got: " + loadBalanceStrategy);
        }
        if (maxConnectionsPerNode < 1) {
            throw new IllegalArgumentException("maxConnectionsPerNode must be >= 1, got: " + maxConnectionsPerNode);
        }
        if (connectionPoolSize < 0) {
            throw new IllegalArgumentException("connectionPoolSize must be >= 0, got: " + connectionPoolSize);
        }
    }
}
