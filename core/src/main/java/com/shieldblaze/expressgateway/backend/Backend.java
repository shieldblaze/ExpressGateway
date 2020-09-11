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
package com.shieldblaze.expressgateway.backend;

import java.net.InetSocketAddress;

public class Backend {
    private InetSocketAddress inetSocketAddress;
    private int weight;
    private int connections;

    public Backend(InetSocketAddress inetSocketAddress, int weight, int connections) {
        this.inetSocketAddress = inetSocketAddress;
        this.weight = weight;
        this.connections = connections;
    }

    public InetSocketAddress getInetSocketAddress() {
        return inetSocketAddress;
    }

    public void setInetSocketAddress(InetSocketAddress inetSocketAddress) {
        this.inetSocketAddress = inetSocketAddress;
    }

    public int getWeight() {
        return weight;
    }

    public void setWeight(int weight) {
        this.weight = weight;
    }

    public int getConnections() {
        return connections;
    }

    public void incConnections() {
        connections++;
    }

    public void decConnections() {
        connections--;
    }

    @Override
    public String toString() {
        return "Backend{" +
                "inetSocketAddress=" + inetSocketAddress +
                ", weight=" + weight +
                ", connections=" + connections +
                '}';
    }
}
