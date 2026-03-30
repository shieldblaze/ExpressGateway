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
package com.shieldblaze.expressgateway.configuration.healthcheck;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;

/**
 * Configuration for per-node circuit breaker.
 *
 * <p>Circuit breaker pattern prevents cascading failures by temporarily
 * stopping traffic to unhealthy backends. Transitions:</p>
 * <pre>
 *   CLOSED --(failureThreshold consecutive failures)--> OPEN
 *   OPEN   --(openDurationMs elapsed)-------------------> HALF_OPEN
 *   HALF_OPEN --(successThreshold consecutive successes)--> CLOSED
 *   HALF_OPEN --(any failure)-----------------------------> OPEN
 * </pre>
 */
public final class CircuitBreakerConfiguration implements Configuration<CircuitBreakerConfiguration> {

    @JsonProperty
    private boolean enabled;

    @JsonProperty
    private int failureThreshold;

    @JsonProperty
    private int successThreshold;

    @JsonProperty
    private long openDurationMs;

    @JsonProperty
    private int halfOpenMaxConcurrent;

    @JsonIgnore
    private boolean validated;

    /**
     * Default: disabled, 5 failures to open, 3 successes to close, 30s open duration
     */
    public static final CircuitBreakerConfiguration DEFAULT = new CircuitBreakerConfiguration();

    static {
        DEFAULT.enabled = false;
        DEFAULT.failureThreshold = 5;
        DEFAULT.successThreshold = 3;
        DEFAULT.openDurationMs = 30_000;
        DEFAULT.halfOpenMaxConcurrent = 1;
        DEFAULT.validated = true;
    }

    CircuitBreakerConfiguration() {
        // Prevent outside initialization
    }

    public CircuitBreakerConfiguration setEnabled(boolean enabled) {
        this.enabled = enabled;
        return this;
    }

    public boolean enabled() {
        assertValidated();
        return enabled;
    }

    /**
     * Number of consecutive failures before the circuit opens
     */
    public CircuitBreakerConfiguration setFailureThreshold(int failureThreshold) {
        this.failureThreshold = failureThreshold;
        return this;
    }

    public int failureThreshold() {
        assertValidated();
        return failureThreshold;
    }

    /**
     * Number of consecutive successes in HALF_OPEN state before closing the circuit
     */
    public CircuitBreakerConfiguration setSuccessThreshold(int successThreshold) {
        this.successThreshold = successThreshold;
        return this;
    }

    public int successThreshold() {
        assertValidated();
        return successThreshold;
    }

    /**
     * Duration in milliseconds to stay in OPEN state before transitioning to HALF_OPEN
     */
    public CircuitBreakerConfiguration setOpenDurationMs(long openDurationMs) {
        this.openDurationMs = openDurationMs;
        return this;
    }

    public long openDurationMs() {
        assertValidated();
        return openDurationMs;
    }

    /**
     * Maximum concurrent requests allowed in HALF_OPEN state
     */
    public CircuitBreakerConfiguration setHalfOpenMaxConcurrent(int halfOpenMaxConcurrent) {
        this.halfOpenMaxConcurrent = halfOpenMaxConcurrent;
        return this;
    }

    public int halfOpenMaxConcurrent() {
        assertValidated();
        return halfOpenMaxConcurrent;
    }

    @Override
    public CircuitBreakerConfiguration validate() {
        NumberUtil.checkPositive(failureThreshold, "FailureThreshold");
        NumberUtil.checkPositive(successThreshold, "SuccessThreshold");
        NumberUtil.checkPositive(openDurationMs, "OpenDurationMs");
        NumberUtil.checkPositive(halfOpenMaxConcurrent, "HalfOpenMaxConcurrent");
        validated = true;
        return this;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
