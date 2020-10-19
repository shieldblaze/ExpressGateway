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
package com.shieldblaze.expressgateway.loadbalance.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.common.eventstream.EventListener;
import com.shieldblaze.expressgateway.common.list.RoundRobinList;
import com.shieldblaze.expressgateway.loadbalance.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;
import java.util.Optional;

/**
 * Select {@link Backend} with least connections with Round-Robin.
 */
public final class LeastConnection extends L4Balance implements EventListener {

    private RoundRobinList<Backend> roundRobinList;

    public LeastConnection() {
        this(new NOOPSessionPersistence());
    }

    public LeastConnection(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public LeastConnection(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence) {
        super(sessionPersistence);
    }

    public LeastConnection(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, Cluster cluster) {
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
    public L4Response getResponse(L4Request l4Request) throws LoadBalanceException {
        Backend backend = sessionPersistence.getBackend(l4Request);
        if (backend != null) {
            return new L4Response(backend);
        }

        // Get Number Of Maximum Connection on a Backend
        int currentMaxConnections = roundRobinList.getList().stream()
                .mapToInt(Backend::getActiveConnections)
                .max()
                .getAsInt();

        // Check If we got any Backend which has less Number of Connections than Backend with Maximum Connection
        Optional<Backend> optionalBackend = roundRobinList.getList().stream()
                .filter(back -> back.getActiveConnections() < currentMaxConnections)
                .findFirst();

        backend = optionalBackend.orElseGet(() -> roundRobinList.iterator().next());
        if (backend == null) {
            throw new LoadBalanceException("No Backend available for Cluster: " + cluster);
        }
        sessionPersistence.addRoute(l4Request.getSocketAddress(), backend);
        return new L4Response(backend);
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
