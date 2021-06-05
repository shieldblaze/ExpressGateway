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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;

import java.net.InetSocketAddress;
import java.net.UnknownHostException;
import java.util.Objects;

/**
 * Builder for {@link Node}
 */
public final class NodeBuilder {

    private Cluster cluster;
    private InetSocketAddress socketAddress;

    private NodeBuilder() {
        // Prevent outside initialization
    }

    public static NodeBuilder newBuilder() {
        return new NodeBuilder();
    }

    public NodeBuilder withCluster(Cluster cluster) {
        this.cluster = cluster;
        return this;
    }

    public NodeBuilder withSocketAddress(InetSocketAddress socketAddress) {
        this.socketAddress = socketAddress;
        return this;
    }

    /**
     * Build {@link Node} Instance
     *
     * @throws NullPointerException If any value is 'null'.
     */
    public Node build() throws NullPointerException, UnknownHostException {
        Objects.requireNonNull(cluster, "Cluster");
        Objects.requireNonNull(socketAddress, "SocketAddress");

        return new Node(cluster, socketAddress);
    }
}
