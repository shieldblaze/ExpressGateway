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

/**
 * Configuration schema for circuit breaker behavior.
 *
 * <p>Circuit breaker pattern prevents cascading failures by temporarily stopping
 * traffic to unhealthy backends. State transitions:</p>
 * <pre>
 *   CLOSED ----(failureThreshold consecutive failures)---> OPEN
 *   OPEN   ----(openDurationMs elapsed)-------------------> HALF_OPEN
 *   HALF_OPEN -(successThreshold consecutive successes)---> CLOSED
 *   HALF_OPEN -(any failure)------------------------------> OPEN
 * </pre>
 *
 * @param enabled              Whether the circuit breaker is enabled
 * @param failureThreshold     Consecutive failures to trip the circuit (must be >= 1 when enabled)
 * @param successThreshold     Consecutive successes in HALF_OPEN to close the circuit (must be >= 1 when enabled)
 * @param halfOpenRequests     Max concurrent requests allowed in HALF_OPEN state (must be >= 1 when enabled)
 * @param openDurationMs       Duration in ms to stay in OPEN before transitioning to HALF_OPEN (must be >= 1 when enabled)
 * @param slidingWindowSize    Sliding window size for failure rate calculation (must be >= 1 when enabled)
 * @param slowCallThresholdPct Percentage of slow calls to consider unhealthy (0-100)
 * @param slowCallDurationMs   Duration in ms above which a call is considered slow (must be >= 0)
 */
public record CircuitBreakerConfig(
        @JsonProperty("enabled") boolean enabled,
        @JsonProperty("failureThreshold") int failureThreshold,
        @JsonProperty("successThreshold") int successThreshold,
        @JsonProperty("halfOpenRequests") int halfOpenRequests,
        @JsonProperty("openDurationMs") long openDurationMs,
        @JsonProperty("slidingWindowSize") int slidingWindowSize,
        @JsonProperty("slowCallThresholdPct") int slowCallThresholdPct,
        @JsonProperty("slowCallDurationMs") long slowCallDurationMs
) {

    /**
     * Validate all fields for correctness.
     *
     * @throws IllegalArgumentException if any field is invalid
     */
    public void validate() {
        if (!enabled) {
            // When disabled, skip threshold validation -- values are irrelevant
            return;
        }
        if (failureThreshold < 1) {
            throw new IllegalArgumentException("failureThreshold must be >= 1 when enabled, got: " + failureThreshold);
        }
        if (successThreshold < 1) {
            throw new IllegalArgumentException("successThreshold must be >= 1 when enabled, got: " + successThreshold);
        }
        if (halfOpenRequests < 1) {
            throw new IllegalArgumentException("halfOpenRequests must be >= 1 when enabled, got: " + halfOpenRequests);
        }
        if (openDurationMs < 1) {
            throw new IllegalArgumentException("openDurationMs must be >= 1 when enabled, got: " + openDurationMs);
        }
        if (slidingWindowSize < 1) {
            throw new IllegalArgumentException("slidingWindowSize must be >= 1 when enabled, got: " + slidingWindowSize);
        }
        if (slowCallThresholdPct < 0 || slowCallThresholdPct > 100) {
            throw new IllegalArgumentException("slowCallThresholdPct must be in range [0, 100], got: " + slowCallThresholdPct);
        }
        if (slowCallDurationMs < 0) {
            throw new IllegalArgumentException("slowCallDurationMs must be >= 0, got: " + slowCallDurationMs);
        }
    }
}
