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
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoBackendAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.algo.roundrobin.RoundRobinIndexGenerator;
import com.shieldblaze.expressgateway.concurrent.event.Event;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

/**
 * Select {@link Node} based on Weight using Round-Robin
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedRoundRobin extends HTTPBalance implements EventListener {

    private RoundRobinIndexGenerator roundRobinIndexGenerator;
    private final TreeRangeMap<Integer, Node> backendsMap = TreeRangeMap.create();
    private int totalWeight;

    public WeightedRoundRobin() {
        super(new NOOPSessionPersistence());
    }

    public WeightedRoundRobin(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public WeightedRoundRobin(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence, Cluster cluster) {
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
        cluster.onlineBackends().forEach(backend -> this.backendsMap.put(Range.closed(totalWeight, totalWeight += backend.weight()), backend));
        roundRobinIndexGenerator = new RoundRobinIndexGenerator(totalWeight);
    }

    @Override
    public HTTPBalanceResponse response(HTTPBalanceRequest request) throws LoadBalanceException {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.node(request);
        if (httpBalanceResponse != null) {
            // If Backend is ONLINE then return the response
            // else remove it from session persistence.
            if (httpBalanceResponse.backend().state() == State.ONLINE) {
                return httpBalanceResponse;
            } else {
                sessionPersistence.removeRoute(request, httpBalanceResponse.backend());
            }
        }

        Node node = backendsMap.get(roundRobinIndexGenerator.next());

        if (node == null) {
            // If Backend is `null` then we don't have any
            // backend to return so we will throw exception.
            throw new NoBackendAvailableException("No Backend available for Cluster: " + cluster);
        } else if (node.state() != State.ONLINE) {
            init(); // We'll reset the mapping because it could be outdated.

            // If selected Backend is not online then
            // we'll throw an exception.
            throw new BackendNotOnlineException("Randomly selected Backend is not online");
        }

        return sessionPersistence.addRoute(request, node);
    }

    @Override
    public void accept(Event event) {
        if (event instanceof BackendEvent) {
            BackendEvent backendEvent = (BackendEvent) event;

        }
    }
}
