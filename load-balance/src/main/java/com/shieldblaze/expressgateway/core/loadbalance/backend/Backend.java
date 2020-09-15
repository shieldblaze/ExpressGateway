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
package com.shieldblaze.expressgateway.core.loadbalance.backend;

import io.netty.util.internal.ObjectUtil;

import java.net.InetSocketAddress;

public class Backend {
    private InetSocketAddress socketAddress;
    private int Weight;
    private int maxConnections;
    private int currentConnections = 0;
    private long bytesWritten = 0L;
    private long bytesReceived = 0L;

    public Backend(InetSocketAddress socketAddress) {
        this(socketAddress, 100, 100_000);
    }

    public Backend(InetSocketAddress socketAddress, int Weight, int maxConnections) {
        ObjectUtil.checkNotNull(socketAddress, "SocketAddress");

        if (Weight < 1) {
            throw new IllegalArgumentException("Weight cannot be less than 1 (one).");
        }

        if (maxConnections < 0) {
            throw new IllegalArgumentException("Maximum Connection cannot be less than 1 (one).");
        }

        this.socketAddress = socketAddress;
        this.Weight = Weight;
        this.maxConnections = maxConnections;
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

    public int getCurrentConnections() {
        return currentConnections;
    }

    public void incConnections() {
        currentConnections++;
    }

    public void decConnections() {
        currentConnections--;
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
}
