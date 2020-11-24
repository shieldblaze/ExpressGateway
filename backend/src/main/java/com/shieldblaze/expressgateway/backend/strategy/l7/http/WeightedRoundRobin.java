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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

import com.google.common.collect.Range;
import com.google.common.collect.TreeRangeMap;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.node.NodeEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedEvent;
import com.shieldblaze.expressgateway.backend.exceptions.NodeOfflineException;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.algo.roundrobin.RoundRobinIndexGenerator;
import com.shieldblaze.expressgateway.concurrent.event.Event;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

/**
 * Select {@link Node} based on Weight using Round-Robin
 */
public final class WeightedRoundRobin extends HTTPBalance {

    private final RoundRobinIndexGenerator roundRobinIndexGenerator = new RoundRobinIndexGenerator(0);

    /**
     * Create {@link WeightedRoundRobin} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    public WeightedRoundRobin(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence) {
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
            node = cluster.nodes().get(roundRobinIndexGenerator.next());
        } catch (Exception ex) {
            throw new NoNodeAvailableException();
        }

        return sessionPersistence.addRoute(request, node);
    }

    @Override
    public void accept(Event event) {
        if (event instanceof NodeEvent) {
            NodeEvent nodeEvent = (NodeEvent) event;
            if (nodeEvent instanceof NodeOfflineEvent || nodeEvent instanceof NodeRemovedEvent || nodeEvent instanceof NodeIdleEvent) {
                sessionPersistence.remove(nodeEvent.node());
                roundRobinIndexGenerator.decMaxIndex();
            } else if (nodeEvent instanceof NodeOnlineEvent) {
                roundRobinIndexGenerator.incMaxIndex();
            }
        }
    }
}
