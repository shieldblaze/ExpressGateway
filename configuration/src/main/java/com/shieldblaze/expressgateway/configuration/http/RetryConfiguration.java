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
package com.shieldblaze.expressgateway.configuration.http;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;
import lombok.Getter;
import lombok.Setter;
import lombok.ToString;
import lombok.experimental.Accessors;

import java.util.EnumSet;
import java.util.Objects;
import java.util.Set;

/**
 * Configuration for upstream request retry with budget control.
 *
 * <p>Inspired by Envoy's retry policy. Retries are budget-limited:
 * at most {@link #retryBudgetPercent} of recent requests may be retries,
 * preventing retry storms under failure conditions.</p>
 */
@Getter
@Setter
@Accessors(fluent = true, chain = true)
@ToString
public final class RetryConfiguration implements Configuration<RetryConfiguration> {

    /**
     * Conditions that trigger a retry attempt
     */
    public enum RetryCondition {
        /** Backend TCP connect failure */
        CONNECT_FAILURE,
        /** Backend returned HTTP 502 */
        HTTP_502,
        /** Backend returned HTTP 503 */
        HTTP_503,
        /** Backend returned HTTP 504 */
        HTTP_504,
        /** Backend connection was reset (RST) */
        RESET,
        /** Backend request timed out */
        TIMEOUT
    }

    @JsonProperty
    private int maxRetries;

    @JsonProperty
    private Set<RetryCondition> retryOn;

    @JsonProperty
    private int retryBudgetPercent;

    @JsonProperty
    private long perTryTimeoutMs;

    /**
     * Default configuration: 2 retries, connect failures and 502/503/504, 20% budget
     */
    public static final RetryConfiguration DEFAULT = new RetryConfiguration();

    static {
        DEFAULT.maxRetries = 2;
        DEFAULT.retryOn = EnumSet.of(
                RetryCondition.CONNECT_FAILURE,
                RetryCondition.HTTP_502,
                RetryCondition.HTTP_503,
                RetryCondition.HTTP_504
        );
        DEFAULT.retryBudgetPercent = 20;
        DEFAULT.perTryTimeoutMs = 0; // 0 means use the global timeout
    }

    RetryConfiguration() {
        // Prevent outside initialization
    }

    @Override
    public RetryConfiguration validate() {
        NumberUtil.checkZeroOrPositive(maxRetries, "MaxRetries");
        Objects.requireNonNull(retryOn, "RetryOn");
        NumberUtil.checkInRange(retryBudgetPercent, 0, 100, "RetryBudgetPercent");
        NumberUtil.checkZeroOrPositive(perTryTimeoutMs, "PerTryTimeoutMs");
        return this;
    }
}
