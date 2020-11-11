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
package com.shieldblaze.expressgateway.loadbalance.l7.http;

import com.google.common.collect.Range;
import com.google.common.collect.TreeRangeMap;
import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.common.algo.roundrobin.RoundRobinIndexGenerator;
import com.shieldblaze.expressgateway.common.eventstream.EventListener;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.loadbalance.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.loadbalance.exceptions.NoBackendAvailableException;

/**
 * Select {@link Backend} based on Weight using Round-Robin
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedRoundRobin extends HTTPBalance implements EventListener {

    private RoundRobinIndexGenerator roundRobinIndexGenerator;
    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private int totalWeight;

    public WeightedRoundRobin() {
        super(new NOOPSessionPersistence());
    }

    public WeightedRoundRobin(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public WeightedRoundRobin(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Backend> sessionPersistence, Cluster cluster) {
        super(sessionPersistence);
        setCluster(cluster);
    }

    @Override
    public void setCluster(Cluster cluster) {
        super.setCluster(cluster);
        init();
        cluster.subscribeStream(this);
    }

    private void init() {
        totalWeight = 0;
        sessionPersistence.clear();
        cluster.getOnlineBackends().forEach(backend -> this.backendsMap.put(Range.closed(totalWeight, totalWeight += backend.getWeight()), backend));
        roundRobinIndexGenerator = new RoundRobinIndexGenerator(totalWeight);
    }

    @Override
    public HTTPBalanceResponse getResponse(HTTPBalanceRequest request) throws LoadBalanceException {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.getBackend(request);
        if (httpBalanceResponse != null) {
            // If Backend is ONLINE then return the response
            // else remove it from session persistence.
            if (httpBalanceResponse.getBackend().getState() == State.ONLINE) {
                return httpBalanceResponse;
            } else {
                sessionPersistence.removeRoute(request, httpBalanceResponse.getBackend());
            }
        }

        Backend backend = backendsMap.get(roundRobinIndexGenerator.next());

        if (backend == null) {
            // If Backend is `null` then we don't have any
            // backend to return so we will throw exception.
            throw new NoBackendAvailableException("No Backend available for Cluster: " + cluster);
        } else if (backend.getState() != State.ONLINE) {
            init(); // We'll reset the mapping because it could be outdated.

            // If selected Backend is not online then
            // we'll throw an exception.
            throw new BackendNotOnlineException("Randomly selected Backend is not online");
        }

        return sessionPersistence.addRoute(request, backend);
    }

    @Override
    public void accept(Object event) {
        if (event instanceof BackendEvent) {
            BackendEvent backendEvent = (BackendEvent) event;
            switch (backendEvent.getType()) {
                case ADDED:
                case ONLINE:
                case OFFLINE:
                case REMOVED:
                    init();
                default:
                    throw new IllegalArgumentException("Unsupported Backend Event Type: " + backendEvent.getType());
            }
        }
    }
}
