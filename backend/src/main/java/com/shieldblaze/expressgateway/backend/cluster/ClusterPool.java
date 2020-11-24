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
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;

import java.util.concurrent.atomic.AtomicInteger;

/**
 * {@linkplain ClusterPool} with multiple {@linkplain Node}
 */
public final class ClusterPool extends Cluster {

    private static final AtomicInteger count = new AtomicInteger();

    public ClusterPool(EventStream eventStream, LoadBalance<?, ?, ?, ?> loadBalance) {
        this(eventStream, loadBalance, "ClusterPool#" + count.getAndIncrement());
    }

    @NonNull
    public ClusterPool(EventStream eventStream, LoadBalance<?, ?, ?, ?> loadBalance, String name) {
        super(eventStream, loadBalance);
        name(name);
    }

    /**
     * @see Cluster#addNode(Node)
     */
    @NonNull
    public void addNodes(Node... nodes) {
        for (Node node : nodes) {
            super.addNode(node);
        }
    }

    @NonNull
    public void addNode(Node node) {
        super.addNode(node);
    }
}
