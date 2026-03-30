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
 * Configuration schema for a retry policy.
 *
 * <p>Defines how failed requests should be retried. Supports status code-based
 * and connection error-based retry triggers with configurable backoff strategies.</p>
 *
 * @param maxRetries      Maximum retry attempts per request (must be >= 0; 0 means no retries)
 * @param retryOn         List of conditions that trigger a retry ("502", "503", "504", "connect-failure", "timeout", "reset")
 * @param backoffStrategy Backoff strategy between retries ("fixed", "exponential", "jittered")
 * @param maxBackoffMs    Maximum backoff duration in milliseconds (must be >= 0)
 * @param perTryTimeoutMs Per-try timeout in milliseconds (must be >= 0; 0 means use the global timeout)
 */
public record RetryPolicyConfig(
        @JsonProperty("maxRetries") int maxRetries,
        @JsonProperty("retryOn") List<String> retryOn,
        @JsonProperty("backoffStrategy") String backoffStrategy,
        @JsonProperty("maxBackoffMs") long maxBackoffMs,
        @JsonProperty("perTryTimeoutMs") long perTryTimeoutMs
) {

    private static final Set<String> VALID_RETRY_CONDITIONS = Set.of(
            "502", "503", "504", "connect-failure", "timeout", "reset"
    );
    private static final Set<String> VALID_BACKOFF_STRATEGIES = Set.of(
            "fixed", "exponential", "jittered"
    );

    public RetryPolicyConfig {
        retryOn = retryOn == null ? List.of() : List.copyOf(retryOn);
    }

    /**
     * Validate all fields for correctness.
     *
     * @throws IllegalArgumentException if any field is invalid
     */
    public void validate() {
        if (maxRetries < 0) {
            throw new IllegalArgumentException("maxRetries must be >= 0, got: " + maxRetries);
        }
        if (maxRetries > 0 && retryOn.isEmpty()) {
            throw new IllegalArgumentException("retryOn must not be empty when maxRetries > 0");
        }
        for (int i = 0; i < retryOn.size(); i++) {
            String condition = retryOn.get(i);
            if (condition == null || !VALID_RETRY_CONDITIONS.contains(condition)) {
                throw new IllegalArgumentException(
                        "retryOn[" + i + "] must be one of " + VALID_RETRY_CONDITIONS + ", got: " + condition);
            }
        }
        Objects.requireNonNull(backoffStrategy, "backoffStrategy");
        if (!VALID_BACKOFF_STRATEGIES.contains(backoffStrategy)) {
            throw new IllegalArgumentException(
                    "backoffStrategy must be one of " + VALID_BACKOFF_STRATEGIES + ", got: " + backoffStrategy);
        }
        if (maxBackoffMs < 0) {
            throw new IllegalArgumentException("maxBackoffMs must be >= 0, got: " + maxBackoffMs);
        }
        if (perTryTimeoutMs < 0) {
            throw new IllegalArgumentException("perTryTimeoutMs must be >= 0, got: " + perTryTimeoutMs);
        }
    }
}
