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

public final class HealthCheckConfigurationBuilder {
    private int workers;
    private int timeInterval;

    private HealthCheckConfigurationBuilder() {
        // Prevent outside initialization
    }

    public static HealthCheckConfigurationBuilder newBuilder() {
        return new HealthCheckConfigurationBuilder();
    }

    public HealthCheckConfigurationBuilder withWorkers(int workers) {
        this.workers = workers;
        return this;
    }

    public HealthCheckConfigurationBuilder withTimeInterval(int timeInterval) {
        this.timeInterval = timeInterval;
        return this;
    }

    public HealthCheckConfiguration build() {
        return new HealthCheckConfiguration()
                .setWorkers(workers)
                .setTimeInterval(timeInterval);
    }
}
