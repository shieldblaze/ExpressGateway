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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.events.node.NodeEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedEvent;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.concurrent.event.Event;

import java.util.SplittableRandom;

/**
 * Select {@link Node} Randomly
 */
public final class HTTPRandom extends HTTPBalance {

    private final SplittableRandom RANDOM = new SplittableRandom();

    /**
     * Create {@link HTTPRandom} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    public HTTPRandom(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence) {
        super(sessionPersistence);
    }

    @Override
    public HTTPBalanceResponse response(HTTPBalanceRequest request) throws LoadBalanceException {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.node(request);
        if (httpBalanceResponse != null) {
            // If Backend is ONLINE then return the response
            // else remove it from session persistence.
            if (httpBalanceResponse.node().state() == State.ONLINE) {
                return httpBalanceResponse;
            } else {
                sessionPersistence.removeRoute(request, httpBalanceResponse.node());
            }
        }

        Node node;
        try {
            node = cluster.nodes().get(RANDOM.nextInt(cluster.nodes().size()));
        } catch (Exception ex) {
            throw new NoNodeAvailableException(ex);
        }

        // Add to session persistence and return
        return sessionPersistence.addRoute(request, node);
    }

    @Override
    public void accept(Event event) {
        if (event instanceof NodeEvent) {
            NodeEvent nodeEvent = (NodeEvent) event;
            if (nodeEvent instanceof NodeOfflineEvent || nodeEvent instanceof NodeRemovedEvent || nodeEvent instanceof NodeIdleEvent) {
                sessionPersistence.remove(nodeEvent.node());
            }
        }
    }
}
