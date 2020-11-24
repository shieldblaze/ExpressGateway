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
import com.shieldblaze.expressgateway.backend.events.node.NodeEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedEvent;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoClusterAvailableException;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.concurrent.event.Event;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

import java.net.InetSocketAddress;
import java.util.Optional;

/**
 * Select {@link Node} with least connections with Round-Robin.
 */
public final class LeastConnection extends L4Balance {

    /**
     * Create {@link LeastConnection} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    public LeastConnection(SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence) {
        super(sessionPersistence);
    }

    @Override
    public L4Response response(L4Request l4Request) throws LoadBalanceException {
        Node node = sessionPersistence.node(l4Request);
        if (node != null) {
            if (node.state() == State.ONLINE) {
                return new L4Response(node);
            } else {
                sessionPersistence.removeRoute(l4Request.socketAddress(), node);
            }
        }

        // Get the Node with least amount of active connections
        Optional<Node> optionalNode = cluster.nodes()
                .stream()
                .reduce((a, b) -> a.activeConnection() < b.activeConnection() ? a : b);

        // If we don't have any node available then throw an exception.
        if (optionalNode.isEmpty()) {
            throw new NoNodeAvailableException();
        }

        node = optionalNode.get();

        sessionPersistence.addRoute(l4Request.socketAddress(), node);
        return new L4Response(node);
    }

    @Override
    public void accept(Event event) {
        if (event instanceof NodeEvent) {
            NodeEvent nodeEvent = (NodeEvent) event;
            if (nodeEvent instanceof NodeOfflineEvent || nodeEvent instanceof NodeRemovedEvent) {
                sessionPersistence.remove(nodeEvent.node());
            }
        }
    }
}
