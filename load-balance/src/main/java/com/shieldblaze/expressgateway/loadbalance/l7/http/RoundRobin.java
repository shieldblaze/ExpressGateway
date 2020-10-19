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

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.common.eventstream.EventListener;
import com.shieldblaze.expressgateway.common.list.RoundRobinList;
import com.shieldblaze.expressgateway.loadbalance.NoBackendAvailableException;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.util.List;

/**
 * Select {@link Backend} based on Round-Robin
 */
public final class RoundRobin extends HTTPBalance implements EventListener {

    private RoundRobinList<Backend> roundRobinList;

    public RoundRobin() {
        super(new NOOPSessionPersistence());
    }

    public RoundRobin(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public RoundRobin(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Backend> sessionPersistence, Cluster cluster) {
        super(sessionPersistence);
        setCluster(cluster);
    }

    @Override
    public void setCluster(Cluster cluster) {
        super.setCluster(cluster);
        roundRobinList = new RoundRobinList<>(cluster.getOnlineBackends());
        cluster.subscribeStream(this);
    }

    @Override
    public HTTPBalanceResponse getResponse(HTTPBalanceRequest httpBalanceRequest) throws NoBackendAvailableException {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.getBackend(httpBalanceRequest);
        if (httpBalanceResponse != null) {
            return httpBalanceResponse;
        }

        Backend backend = roundRobinList.iterator().next();

        if (backend == null) {
            throw new NoBackendAvailableException("No Backend available for Cluster: " + cluster);
        }

        return sessionPersistence.addRoute(httpBalanceRequest, backend);
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
                    roundRobinList.newIterator(cluster.getOnlineBackends());
                default:
                    throw new IllegalArgumentException("Unsupported Backend Event Type: " + backendEvent.getType());
            }
        }
    }
}
