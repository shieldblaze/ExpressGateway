/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;

import java.io.IOException;

public class HealthCheckConfiguration {

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
        NumberUtil.checkPositive(workers, "Workers");
        this.workers = workers;
        return this;
    }

    HealthCheckConfiguration setTimeInterval(int timeInterval) {
        NumberUtil.checkPositive(timeInterval, "TimeInterval");
        this.timeInterval = timeInterval;
        return this;
    }

    public HealthCheckConfiguration validate() {
        NumberUtil.checkPositive(workers, "Workers");
        NumberUtil.checkPositive(timeInterval, "TimeInterval");
        return this;
    }

    /**
     * Save this configuration to the file
     *
     * @throws IOException If an error occurs during saving
     */
    public void save() throws IOException {
        ConfigurationMarshaller.save("HealthCheckConfiguration.json", this);
    }

    /**
     * Load a configuration
     *
     * @return {@link HealthCheckConfiguration} Instance
     */
    public static HealthCheckConfiguration load() {
        try {
            return ConfigurationMarshaller.load("HealthCheckConfiguration.json", HealthCheckConfiguration.class);
        } catch (Exception ex) {
            // Ignore
        }
        return DEFAULT;
    }
}
