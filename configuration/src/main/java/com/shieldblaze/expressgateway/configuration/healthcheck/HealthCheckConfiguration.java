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

import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;

import java.io.IOException;

public class HealthCheckConfiguration extends ConfigurationMarshaller {

    @Expose
    private final int workers;

    @Expose
    private final int timeInterval;

    public static final HealthCheckConfiguration DEFAULT = new HealthCheckConfiguration(
            Runtime.getRuntime().availableProcessors(),
            1000 * 10 // 10 Seconds
    );

    HealthCheckConfiguration(int workers, int timeInterval) {
        this.workers = workers;
        this.timeInterval = timeInterval;
    }

    public int workers() {
        return workers;
    }

    public int timeInterval() {
        return timeInterval;
    }

    public static HealthCheckConfiguration loadFrom() throws IOException {
        return loadFrom(HealthCheckConfiguration.class, "HealthCheck.json");
    }

    public void saveTo() throws IOException {
        saveTo(this, "HealthCheck.json");
    }
}
