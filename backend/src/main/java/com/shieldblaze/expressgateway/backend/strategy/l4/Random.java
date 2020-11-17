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
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoBackendAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.Event;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

import java.net.InetSocketAddress;

/**
 * Select {@link Node} Randomly
 */
public final class Random extends L4Balance implements EventListener {
    private final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    public Random() {
        super(new NOOPSessionPersistence());
    }

    public Random(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public Random(SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence, Cluster cluster) {
        super(sessionPersistence);
        cluster(cluster);
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

        int index = RANDOM_INSTANCE.nextInt(cluster.online());

        try {
            node = cluster.online(index);
        } catch (BackendNotOnlineException e) {
            // If selected Backend is not online then
            // we'll throw an exception. However, this should
            // rarely or never happen in most cases.
            throw new NoBackendAvailableException("Randomly selected Backend is not online");
        }

        // Add to session persistence
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
