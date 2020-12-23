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
package com.shieldblaze.expressgateway.configuration.controller;

import com.shieldblaze.expressgateway.common.utils.Number;

/**
 * Builder for {@link ControllerConfiguration}
 */
public final class ControllerConfigurationBuilder {

    private int workers;
    private long healthCheckIntervalMilliseconds;
    private long deadConnectionCleanIntervalMilliseconds;
    private long ocspCheckIntervalMilliseconds;

    private ControllerConfigurationBuilder() {
        // Prevent outside initialization
    }

    public static ControllerConfigurationBuilder newBuilder() {
        return new ControllerConfigurationBuilder();
    }

    public ControllerConfigurationBuilder withWorkers(int workers) {
        this.workers = workers;
        return this;
    }

    public ControllerConfigurationBuilder withHealthCheckIntervalMilliseconds(long healthCheckIntervalMilliseconds) {
        this.healthCheckIntervalMilliseconds = healthCheckIntervalMilliseconds;
        return this;
    }

    public ControllerConfigurationBuilder withDeadConnectionCleanIntervalMilliseconds(long deadConnectionCleanIntervalMilliseconds) {
        this.deadConnectionCleanIntervalMilliseconds = deadConnectionCleanIntervalMilliseconds;
        return this;
    }

    public ControllerConfigurationBuilder withOCSPCheckIntervalMilliseconds(long ocspCheckIntervalMilliseconds) {
        this.ocspCheckIntervalMilliseconds = ocspCheckIntervalMilliseconds;
        return this;
    }

    public ControllerConfiguration build() {
        Number.checkPositive(workers, "Workers");
        Number.checkZeroOrPositive(healthCheckIntervalMilliseconds, "HealthCheckIntervalMilliseconds");
        Number.checkPositive(deadConnectionCleanIntervalMilliseconds, "DeadConnectionCleanIntervalMilliseconds");
        Number.checkZeroOrPositive(ocspCheckIntervalMilliseconds, "OCSPCheckIntervalMilliseconds");
        return new ControllerConfiguration(workers, healthCheckIntervalMilliseconds, deadConnectionCleanIntervalMilliseconds, ocspCheckIntervalMilliseconds);
    }
}
