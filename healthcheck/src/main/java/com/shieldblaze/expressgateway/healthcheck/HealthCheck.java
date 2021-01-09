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
package com.shieldblaze.expressgateway.healthcheck;

import com.google.common.collect.EvictingQueue;
import com.shieldblaze.expressgateway.common.Math;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Collections;

/**
 * Health Check for checking health of remote host.
 */
@SuppressWarnings("UnstableApiUsage")
public abstract class HealthCheck implements Runnable {

    protected final InetSocketAddress socketAddress;
    private final EvictingQueue<Boolean> queue;
    protected final int timeout;

    /**
     * Create a new {@link HealthCheck} Instance with {@code samples} set to 100.
     *
     * @param socketAddress {@link InetSocketAddress} of remote host to check
     * @param timeout       Timeout in seconds for health check
     */
    public HealthCheck(InetSocketAddress socketAddress, Duration timeout) {
        this(socketAddress, timeout, 100);
    }

    /**
     * Create a new {@link HealthCheck} Instance
     *
     * @param socketAddress {@link InetSocketAddress} of remote host to check
     * @param timeout       Timeout for health check
     * @param samples       Number of samples to use for evaluating Health of remote host
     */
    public HealthCheck(InetSocketAddress socketAddress, Duration timeout, int samples) {
        this.socketAddress = socketAddress;
        this.timeout = (int) timeout.toMillis();
        this.queue = EvictingQueue.create(samples);
    }

    /**
     * If Heath Check was successful, call this method.
     */
    protected void markSuccess() {
        queue.add(true);
    }

    /**
     * If Heath Check was unsuccessful, call this method.
     */
    protected void markFailure() {
        queue.add(false);
    }

    /**
     * Get {@link Health} of Remote Host
     */
    public Health health() {
        double percentage = Math.percentage(Collections.frequency(queue, true), queue.size());
        if (percentage >= 95) {
            return Health.GOOD;
        } else if (percentage >= 75) {
            return Health.MEDIUM;
        } else {
            return Health.BAD;
        }
    }
}
