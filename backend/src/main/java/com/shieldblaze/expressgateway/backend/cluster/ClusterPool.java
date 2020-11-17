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

import java.util.Objects;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * {@linkplain ClusterPool} with multiple {@linkplain Node}
 */
public final class ClusterPool extends Cluster {

    private static final AtomicInteger count = new AtomicInteger();

    public ClusterPool() {
        name("ClusterPool#" + count.getAndIncrement());
    }

    private ClusterPool(String name, String hostname, Node... nodes) {
        name(name);
        hostname(hostname);
        addBackends(nodes);
    }

    public static ClusterPool of(String hostname, Node... nodes) {
        return new ClusterPool("ClusterPool#" + count.getAndIncrement(), hostname, nodes);
    }

    public static ClusterPool of(String name, String hostname, Node... nodes) {
        return new ClusterPool(name, hostname, nodes);
    }

    /**
     * @see Cluster#addBackend(Node)
     */
    public void addBackends(Node... nodes) {
        Objects.requireNonNull(nodes, "backends");
        for (Node node : nodes) {
            super.addBackend(node);
        }
    }

    public void addBackend(Node backends) {
        super.addBackend(backends);
    }
}
