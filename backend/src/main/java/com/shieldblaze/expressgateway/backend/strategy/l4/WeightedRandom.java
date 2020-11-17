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

import com.google.common.collect.Range;
import com.google.common.collect.TreeRangeMap;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.Event;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

import java.net.InetSocketAddress;

/**
 * Select {@link Node} based on Weight Randomly
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedRandom extends L4Balance implements EventListener {
    private final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    private final TreeRangeMap<Integer, Node> backendsMap = TreeRangeMap.create();
    private int totalWeight;

    public WeightedRandom() {
        super(new NOOPSessionPersistence());
    }

    public WeightedRandom(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public WeightedRandom(SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence, Cluster cluster) {
        super(sessionPersistence);
        cluster(cluster);
    }

    @Override
    public void cluster(Cluster cluster) {
        super.cluster(cluster);
        init();
        cluster.subscribeStream(this);
    }

    private void init() {
        totalWeight = 0;
        sessionPersistence.clear();
        backendsMap.clear();
        cluster.onlineBackends().forEach(backend -> backendsMap.put(Range.closed(totalWeight, totalWeight += backend.weight()), backend));
    }

    @Override
    public L4Response response(L4Request l4Request) throws LoadBalanceException {
        Node node = sessionPersistence.node(l4Request);
        if (node != null) {
            // If Backend is ONLINE then return the response
            // else remove it from session persistence.
            if (node.state() == State.ONLINE) {
                return new L4Response(node);
            } else {
                sessionPersistence.removeRoute(l4Request.socketAddress(), node);
            }
        }

        int index = RANDOM_INSTANCE.nextInt(totalWeight);
        node = backendsMap.get(index);

        if (node == null) {
            // If Backend is `null` then we don't have any
            // backend to return so we will throw exception.
            throw new LoadBalanceException();
        } else if (node.state() != State.ONLINE) {
            init(); // We'll reset the mapping because it could be outdated.

            // If selected Backend is not online then
            // we'll throw an exception.
            throw new LoadBalanceException();
        }

        sessionPersistence.addRoute(l4Request.socketAddress(), node);
        return new L4Response(node);
    }

    @Override
    public void accept(Event event) {
        if (event instanceof BackendEvent) {
            BackendEvent backendEvent = (BackendEvent) event;

        }
    }
}
