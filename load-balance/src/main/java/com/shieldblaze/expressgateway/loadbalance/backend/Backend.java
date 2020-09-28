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
package com.shieldblaze.expressgateway.loadbalance.backend;

import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import io.netty.util.internal.ObjectUtil;

import java.net.InetSocketAddress;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * {@link Backend} is the server which handles actual request of client.
 */
public class Backend {

    /**
     * Address of this {@link Backend}
     */
    private InetSocketAddress socketAddress;

    /**
     * Weight of this {@link Backend}
     */
    private int Weight;

    /**
     * Maximum Number Of Connections Allowed for this {@link Backend}
     */
    private int maxConnections;

    /**
     * Active Number Of Connection for this {@link Backend}
     */
    private final AtomicInteger activeConnections = new AtomicInteger();

    /**
     * Number of bytes written so far to this {@link Backend}
     */
    private long bytesWritten = 0L;

    /**
     * Number of bytes received so far from this {@link Backend}
     */
    private long bytesReceived = 0L;

    /**
     * Current State of this {@link Backend}
     */
    private State state;

    /**
     * Health Check for this {@link Backend}
     */
    private HealthCheck healthCheck;

    /**
     * Create {@link Backend} with {@code Weight 100} and {@code maxConnections 10000}
     *
     * @param socketAddress Address of this {@link Backend}
     */
    public Backend(InetSocketAddress socketAddress) {
        this(socketAddress, 100, 10_000, null);
    }

    public Backend(InetSocketAddress socketAddress, int Weight, int maxConnections, HealthCheck healthCheck) {
        ObjectUtil.checkNotNull(socketAddress, "SocketAddress");

        if (Weight < 1) {
            throw new IllegalArgumentException("Weight cannot be less than 1 (one).");
        }

        if (maxConnections < 0) {
            throw new IllegalArgumentException("Maximum Connection cannot be less than 1 (one).");
        }

        this.state = State.ONLINE;
        this.socketAddress = socketAddress;
        this.Weight = Weight;
        this.maxConnections = maxConnections;
        this.healthCheck = healthCheck;
    }

    public InetSocketAddress getSocketAddress() {
        return socketAddress;
    }

    public void setSocketAddress(InetSocketAddress socketAddress) {
        this.socketAddress = socketAddress;
    }

    public int getWeight() {
        return Weight;
    }

    public void setWeight(int weight) {
        this.Weight = weight;
    }

    public int getActiveConnections() {
        return activeConnections.get();
    }

    public void incConnections() {
        activeConnections.incrementAndGet();
    }

    public void decConnections() {
        activeConnections.decrementAndGet();
    }

    public void incBytesWritten(int bytes) {
        bytesWritten += bytes;
    }

    public void incBytesReceived(int bytes) {
        bytesReceived += bytes;
    }

    public void setMaxConnections(int maxConnections) {
        this.maxConnections = maxConnections;
    }

    public int getMaxConnections() {
        return maxConnections;
    }

    public long getBytesWritten() {
        return bytesWritten;
    }

    public long getBytesReceived() {
        return bytesReceived;
    }

    public State getState() {
        return state;
    }

    public void setState(State state) {
        this.state = state;
    }

    public HealthCheck getHealthCheck() {
        return healthCheck;
    }

    public void setHealthCheck(HealthCheck healthCheck) {
        this.healthCheck = healthCheck;
    }

    public Health getHealth() {
        if (healthCheck == null) {
            return Health.UNKNOWN;
        }
        return healthCheck.health();
    }
}
