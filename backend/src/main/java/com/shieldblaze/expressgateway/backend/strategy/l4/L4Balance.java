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
package com.shieldblaze.expressgateway.backend.strategy.l4;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

import java.net.InetSocketAddress;

/**
 * <p> Balance Layer-4 Traffic using the available methods: </p>
 * <ul>
 *     <li> {@link LeastConnection} </li>
 *     <li> {@link Random} </li>
 *     <li> {@link RoundRobin} </li>
 *     <li> {@link WeightedLeastConnection} </li>
 *     <li> {@link WeightedRoundRobin} </li>
 * </ul>
 */
public abstract class L4Balance extends LoadBalance<Node, Node, InetSocketAddress, Node> {

    /**
     * Create {@link L4Balance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    @NonNull
    public L4Balance(SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence) {
        super(sessionPersistence);
    }

    @NonNull
    @Override
    public void cluster(Cluster cluster) {
        super.cluster(cluster);
    }

    @NonNull
    public abstract L4Response response(L4Request l4Request) throws LoadBalanceException;

    @Override
    public Response response(Request request) throws LoadBalanceException {
        return response((L4Request) request);
    }
}
