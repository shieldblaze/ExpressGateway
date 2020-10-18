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
package com.shieldblaze.expressgateway.backend.healthcheckmanager;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import io.netty.util.internal.ObjectUtil;

import java.util.Objects;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

public abstract class HealthCheckManager implements Runnable {

    protected Backend backend;
    protected final HealthCheck healthCheck;
    private final int initialDelay;
    private final int time;
    private final TimeUnit timeUnit;

    private ScheduledFuture<?> scheduledFutureHealthCheck;

    public HealthCheckManager(HealthCheck healthCheck, int initialDelay, int time, TimeUnit timeUnit) {
        this.healthCheck = Objects.requireNonNull(healthCheck);
        this.time = ObjectUtil.checkPositive(time, "time");
        this.initialDelay = ObjectUtil.checkPositive(initialDelay, "initialDelay");
        this.timeUnit = Objects.requireNonNull(timeUnit, "timeUnit");
    }

    public void initialize() {
       if (scheduledFutureHealthCheck == null) {
           scheduledFutureHealthCheck = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(healthCheck, initialDelay, time, timeUnit);
       }
    }

    public void shutdown() {
        if (scheduledFutureHealthCheck != null) {
            scheduledFutureHealthCheck.cancel(true);
            scheduledFutureHealthCheck = null;
        }
    }

    public void setBackend(Backend backend) {
        this.backend = Objects.requireNonNull(backend);
    }
}
