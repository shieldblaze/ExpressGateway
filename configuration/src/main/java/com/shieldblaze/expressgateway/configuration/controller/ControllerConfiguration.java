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

public final class ControllerConfiguration {

    /**
     * Numbers of dedicated workers (threads) to use for controlling and managing.
     */
    private final int workers;

    /**
     * Number of milliseconds to wait before performing Health Check.
     */
    private final long healthCheckIntervalMilliseconds;

    /**
     * Number of milliseconds to wait before performing Dead Connection Cleanup.
     */
    private final long deadConnectionCleanIntervalMilliseconds;

    /**
     * Number of milliseconds to wait before fetching OCSP Data for OCSP Stapling.
     */
    private final long ocspCheckIntervalMilliseconds;

    ControllerConfiguration(int workers, long healthCheckIntervalMilliseconds, long deadConnectionCleanIntervalMilliseconds, long ocspCheckIntervalMilliseconds) {
        this.workers = workers;
        this.healthCheckIntervalMilliseconds = healthCheckIntervalMilliseconds;
        this.deadConnectionCleanIntervalMilliseconds = deadConnectionCleanIntervalMilliseconds;
        this.ocspCheckIntervalMilliseconds = ocspCheckIntervalMilliseconds;
    }

    public int workers() {
        return workers;
    }

    public long healthCheckIntervalMilliseconds() {
        return healthCheckIntervalMilliseconds;
    }

    public long deadConnectionCleanIntervalMilliseconds() {
        return deadConnectionCleanIntervalMilliseconds;
    }

    public long ocspCheckIntervalMilliseconds() {
        return ocspCheckIntervalMilliseconds;
    }
}
