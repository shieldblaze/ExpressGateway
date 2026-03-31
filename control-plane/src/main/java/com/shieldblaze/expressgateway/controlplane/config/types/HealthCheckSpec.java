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
 * Configuration spec for health check probes.
 *
 * @param name               The health check name
 * @param type               The probe type ("tcp", "http", "udp")
 * @param intervalSeconds    Interval between probes in seconds
 * @param timeoutSeconds     Per-probe timeout in seconds
 * @param healthyThreshold   Number of consecutive successes to mark healthy
 * @param unhealthyThreshold Number of consecutive failures to mark unhealthy
 * @param httpPath           HTTP path for HTTP probes (optional, only for type "http")
 * @param expectedStatusCode Expected HTTP status code for HTTP probes (optional, only for type "http")
 */
public record HealthCheckSpec(
        @JsonProperty("name") String name,
        @JsonProperty("type") String type,
        @JsonProperty("intervalSeconds") int intervalSeconds,
        @JsonProperty("timeoutSeconds") int timeoutSeconds,
        @JsonProperty("healthyThreshold") int healthyThreshold,
        @JsonProperty("unhealthyThreshold") int unhealthyThreshold,
        @JsonProperty("httpPath") String httpPath,
        @JsonProperty("expectedStatusCode") int expectedStatusCode
) implements ConfigSpec {

    private static final Set<String> VALID_TYPES = Set.of("tcp", "http", "udp");

    @Override
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        Objects.requireNonNull(type, "type");
        if (!VALID_TYPES.contains(type)) {
            throw new IllegalArgumentException("type must be one of " + VALID_TYPES + ", got: " + type);
        }
        if (intervalSeconds < 1) {
            throw new IllegalArgumentException("intervalSeconds must be >= 1, got: " + intervalSeconds);
        }
        if (timeoutSeconds < 1) {
            throw new IllegalArgumentException("timeoutSeconds must be >= 1, got: " + timeoutSeconds);
        }
        if (timeoutSeconds >= intervalSeconds) {
            throw new IllegalArgumentException(
                    "timeoutSeconds (" + timeoutSeconds + ") must be less than intervalSeconds (" + intervalSeconds + ")");
        }
        if (healthyThreshold < 1) {
            throw new IllegalArgumentException("healthyThreshold must be >= 1, got: " + healthyThreshold);
        }
        if (unhealthyThreshold < 1) {
            throw new IllegalArgumentException("unhealthyThreshold must be >= 1, got: " + unhealthyThreshold);
        }
        // HTTP-specific validation
        if ("http".equals(type)) {
            if (httpPath == null || httpPath.isBlank()) {
                throw new IllegalArgumentException("httpPath is required when type is 'http'");
            }
            if (!httpPath.startsWith("/")) {
                throw new IllegalArgumentException("httpPath must start with '/', got: " + httpPath);
            }
            if (expectedStatusCode < 100 || expectedStatusCode > 599) {
                throw new IllegalArgumentException(
                        "expectedStatusCode must be in range [100, 599], got: " + expectedStatusCode);
            }
        }
    }
}
