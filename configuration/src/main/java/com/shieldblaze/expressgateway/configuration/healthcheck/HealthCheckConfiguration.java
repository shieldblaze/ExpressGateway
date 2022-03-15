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

/**
 * Configuration for {@link HealthCheckConfiguration}.
 * <p>
 * Use {@link HealthCheckConfigurationBuilder} to build {@link HealthCheckConfiguration} instance.
 */
public final class HealthCheckConfiguration implements Configuration {

    @JsonProperty(value = "workers")
    private int workers;

    @JsonProperty(value = "timeInterval")
    private int timeInterval;

    public static final HealthCheckConfiguration DEFAULT = new HealthCheckConfiguration()
            .setWorkers(Runtime.getRuntime().availableProcessors())
            .setTimeInterval(1); // 1 Second

    public int workers() {
        return workers;
    }

    public int timeInterval() {
        return timeInterval;
    }

    HealthCheckConfiguration setWorkers(int workers) {
        this.workers = workers;
        return this;
    }

    HealthCheckConfiguration setTimeInterval(int timeInterval) {
        this.timeInterval = timeInterval;
        return this;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    public HealthCheckConfiguration validate() throws IllegalArgumentException {
        NumberUtil.checkPositive(workers, "Workers");
        NumberUtil.checkPositive(timeInterval, "TimeInterval");
        return this;
    }

    @Override
    public String name() {
        return "HealthCheckConfiguration";
    }
}
