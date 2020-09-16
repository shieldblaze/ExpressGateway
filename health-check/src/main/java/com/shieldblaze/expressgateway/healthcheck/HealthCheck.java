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
package com.shieldblaze.expressgateway.healthcheck;

import com.google.common.collect.EvictingQueue;

import java.net.InetSocketAddress;
import java.util.Collections;

@SuppressWarnings("UnstableApiUsage")
public abstract class HealthCheck {

    protected final InetSocketAddress socketAddress;
    private EvictingQueue<Boolean> queue;
    protected final int timeout;

    public HealthCheck(InetSocketAddress socketAddress, int timeout) {
        this(socketAddress, timeout, 100);
    }

    public HealthCheck(InetSocketAddress socketAddress, int timeout, int samples) {
        this.socketAddress = socketAddress;
        this.timeout = timeout;
        this.queue = EvictingQueue.create(samples);
    }

    public abstract void check();

    protected void markSuccess() {
        queue.add(true);
    }

    protected void markFailure() {
        queue.add(false);
    }

    public Health getHealth() {
        double percentage = getPercentage(Collections.frequency(queue, true), queue.size());
        if (percentage >= 95) {
            return Health.GOOD;
        } else if (percentage >= 75) {
            return Health.MEDIUM;
        } else {
            return Health.BAD;
        }
    }

    private double getPercentage(double num, double total) {
        return num * 100 / total;
    }
}
