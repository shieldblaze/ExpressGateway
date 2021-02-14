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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;

/**
 * {@linkplain ClusterPool} is the basic implementation of {@linkplain Cluster}
 */
public final class ClusterPool extends Cluster {

    public ClusterPool(LoadBalance<?, ?, ?, ?> loadBalance) {
        super(loadBalance);
    }

    /**
     * @see Cluster#addNode(Node)
     */
    @NonNull
    public void addNodes(Node... nodes) {
        for (Node node : nodes) {
            addNode(node);
        }
    }

    @NonNull
    public boolean addNode(Node node) {
        return super.addNode(node);
    }
}
