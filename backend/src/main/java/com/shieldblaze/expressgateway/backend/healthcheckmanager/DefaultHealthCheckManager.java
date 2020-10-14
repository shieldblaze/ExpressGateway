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

import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.common.concurrent.GlobalEventExecutors;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

public final class DefaultHealthCheckManager extends HealthCheckManager {

    private ScheduledFuture<?> scheduledFutureHealthCheck;

    public DefaultHealthCheckManager(HealthCheck healthCheck, int initialDelay, int time, TimeUnit timeUnit) {
        super(healthCheck, initialDelay, time, timeUnit);
        this.scheduledFutureHealthCheck = GlobalEventExecutors.INSTANCE.submitTaskAndRunEvery(this, initialDelay, time, timeUnit);
    }

    @Override
    public void run() {
        if (healthCheck.health() == Health.GOOD) {
            backend.setState(State.ONLINE);
        } else {
            backend.setState(State.OFFLINE);
        }
    }

    @Override
    public void shutdown() {
        if (this.scheduledFutureHealthCheck != null) {
            this.scheduledFutureHealthCheck.cancel(true);
            this.scheduledFutureHealthCheck = null;
            super.shutdown();
        }
    }
}
