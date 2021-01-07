/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.common.utils.Number;

public class HealthCheckConfiguration implements Configuration {

    public static final HealthCheckConfiguration EMPTY_INSTANCE = new HealthCheckConfiguration();

    private HealthCheckConfiguration() {
        // Prevent outside initialization
    }

    @Expose
    @JsonProperty("workers")
    private int workers;

    @Expose
    @JsonProperty("timeInterval")
    private int timeInterval;

    public HealthCheckConfiguration(int workers, int timeInterval) {
        this.workers = workers;
        this.timeInterval = timeInterval;
    }

    public int workers() {
        return workers;
    }

    public int timeInterval() {
        return timeInterval;
    }

    @Override
    public String name() {
        return "HealthCheck";
    }

    @Override
    public void validate() {
        Number.checkPositive(workers, "Workers");
        Number.checkPositive(timeInterval, "Time Interval");
    }
}
