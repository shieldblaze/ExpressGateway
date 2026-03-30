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
package com.shieldblaze.expressgateway.controlplane.config.types;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;

import java.util.Objects;
import java.util.Set;

/**
 * Configuration spec for a backend cluster.
 *
 * @param name                The cluster name
 * @param loadBalanceStrategy The load balancing strategy (e.g. "round-robin", "least-connection")
 * @param healthCheckName     Reference to a {@link HealthCheckSpec} by name
 * @param maxConnections      Maximum total connections to this cluster
 * @param drainTimeoutSeconds Drain timeout in seconds for graceful removal of nodes
 */
public record ClusterSpec(
        @JsonProperty("name") String name,
        @JsonProperty("loadBalanceStrategy") String loadBalanceStrategy,
        @JsonProperty("healthCheckName") String healthCheckName,
        @JsonProperty("maxConnections") int maxConnections,
        @JsonProperty("drainTimeoutSeconds") int drainTimeoutSeconds
) implements ConfigSpec {

    private static final Set<String> VALID_STRATEGIES = Set.of(
            "round-robin", "least-connection", "random", "ip-hash", "weighted-round-robin"
    );

    @Override
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        Objects.requireNonNull(loadBalanceStrategy, "loadBalanceStrategy");
        if (!VALID_STRATEGIES.contains(loadBalanceStrategy)) {
            throw new IllegalArgumentException(
                    "loadBalanceStrategy must be one of " + VALID_STRATEGIES + ", got: " + loadBalanceStrategy);
        }
        Objects.requireNonNull(healthCheckName, "healthCheckName");
        if (healthCheckName.isBlank()) {
            throw new IllegalArgumentException("healthCheckName must not be blank");
        }
        if (maxConnections < 1) {
            throw new IllegalArgumentException("maxConnections must be >= 1, got: " + maxConnections);
        }
        if (drainTimeoutSeconds < 0) {
            throw new IllegalArgumentException("drainTimeoutSeconds must be >= 0, got: " + drainTimeoutSeconds);
        }
    }
}
