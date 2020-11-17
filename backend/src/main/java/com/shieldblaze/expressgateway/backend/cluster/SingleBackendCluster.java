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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.Node;

import java.util.concurrent.atomic.AtomicInteger;

/**
 * {@linkplain Cluster} with single {@linkplain Node}
 */
public final class SingleBackendCluster extends Cluster {

    private static final AtomicInteger count = new AtomicInteger();

    private SingleBackendCluster(String name, String hostname, Node node) {
        name(name);
        hostname(hostname);
        addNode(node);
    }

    public static SingleBackendCluster of(Node node) {
        return new SingleBackendCluster("SingleBackendCluster#" + count.getAndIncrement(), node.socketAddress().getHostName(), node);
    }

    public static SingleBackendCluster of(String hostname, Node node) {
        return new SingleBackendCluster("SingleBackendCluster#" + count.getAndIncrement(), hostname, node);
    }

    public static SingleBackendCluster of(String name, String hostname, Node node) {
        return new SingleBackendCluster(name, hostname, node);
    }
}
