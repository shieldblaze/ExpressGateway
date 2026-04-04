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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;
import lombok.Getter;
import lombok.Setter;
import lombok.ToString;
import lombok.experimental.Accessors;

/**
 * Configuration for health check scheduling.
 */
@Getter
@Setter
@Accessors(fluent = true, chain = true)
@ToString
public final class HealthCheckConfiguration implements Configuration<HealthCheckConfiguration> {

    @JsonProperty
    private int workers;

    @JsonProperty
    private int timeInterval;

    public static final HealthCheckConfiguration DEFAULT = new HealthCheckConfiguration();

    static {
        DEFAULT.workers = Runtime.getRuntime().availableProcessors();
        DEFAULT.timeInterval = 1;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    @Override
    public HealthCheckConfiguration validate() {
        NumberUtil.checkPositive(workers, "Workers");
        NumberUtil.checkPositive(timeInterval, "TimeInterval");
        return this;
    }
}
