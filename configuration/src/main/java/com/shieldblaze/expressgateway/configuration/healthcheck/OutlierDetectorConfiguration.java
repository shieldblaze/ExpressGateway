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
import lombok.ToString;

/**
 * Configuration for passive health checking (outlier detection).
 *
 * <p>Outlier detection passively monitors backend responses and ejects
 * backends that exceed a failure threshold. After ejection time passes,
 * the backend is allowed back (set to IDLE for active health check to verify).</p>
 *
 * <p>Similar to Envoy's outlier detection feature.</p>
 */
@ToString(exclude = "validated")
public final class OutlierDetectorConfiguration implements Configuration<OutlierDetectorConfiguration> {

    @JsonProperty
    private boolean enabled;

    @JsonProperty
    private int consecutiveFailures;

    @JsonProperty
    private long intervalMs;

    @JsonProperty
    private long ejectionTimeMs;

    @JsonProperty
    private int maxEjectionPercent;

    @JsonIgnore
    private boolean validated;

    /**
     * Default: disabled, 5 consecutive failures, 10s interval, 30s ejection, max 50% ejected
     */
    public static final OutlierDetectorConfiguration DEFAULT = new OutlierDetectorConfiguration();

    static {
        DEFAULT.enabled = false;
        DEFAULT.consecutiveFailures = 5;
        DEFAULT.intervalMs = 10_000;
        DEFAULT.ejectionTimeMs = 30_000;
        DEFAULT.maxEjectionPercent = 50;
        DEFAULT.validated = true;
    }

    OutlierDetectorConfiguration() {
        // Prevent outside initialization
    }

    public OutlierDetectorConfiguration setEnabled(boolean enabled) {
        this.enabled = enabled;
        return this;
    }

    public boolean enabled() {
        assertValidated();
        return enabled;
    }

    /**
     * Number of consecutive failures before ejecting a backend
     */
    public OutlierDetectorConfiguration setConsecutiveFailures(int consecutiveFailures) {
        this.consecutiveFailures = consecutiveFailures;
        return this;
    }

    public int consecutiveFailures() {
        assertValidated();
        return consecutiveFailures;
    }

    /**
     * Evaluation window interval in milliseconds
     */
    public OutlierDetectorConfiguration setIntervalMs(long intervalMs) {
        this.intervalMs = intervalMs;
        return this;
    }

    public long intervalMs() {
        assertValidated();
        return intervalMs;
    }

    /**
     * Duration in milliseconds a backend stays ejected before being allowed back
     */
    public OutlierDetectorConfiguration setEjectionTimeMs(long ejectionTimeMs) {
        this.ejectionTimeMs = ejectionTimeMs;
        return this;
    }

    public long ejectionTimeMs() {
        assertValidated();
        return ejectionTimeMs;
    }

    /**
     * Maximum percentage of backends that can be ejected simultaneously (0-100).
     * Prevents ejecting all backends during a widespread issue.
     */
    public OutlierDetectorConfiguration setMaxEjectionPercent(int maxEjectionPercent) {
        this.maxEjectionPercent = maxEjectionPercent;
        return this;
    }

    public int maxEjectionPercent() {
        assertValidated();
        return maxEjectionPercent;
    }

    @Override
    public OutlierDetectorConfiguration validate() {
        NumberUtil.checkPositive(consecutiveFailures, "ConsecutiveFailures");
        NumberUtil.checkPositive(intervalMs, "IntervalMs");
        NumberUtil.checkPositive(ejectionTimeMs, "EjectionTimeMs");
        NumberUtil.checkInRange(maxEjectionPercent, 1, 100, "MaxEjectionPercent");
        validated = true;
        return this;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
