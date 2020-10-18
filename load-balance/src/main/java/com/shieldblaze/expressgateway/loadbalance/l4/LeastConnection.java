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
import com.shieldblaze.expressgateway.common.list.RoundRobinList;
import com.shieldblaze.expressgateway.loadbalance.NoBackendAvailableException;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;
import java.util.Optional;

/**
 * Select {@link Backend} with least connections with Round-Robin.
 */
public final class LeastConnection extends L4Balance {

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
    public L4Response getResponse(L4Request l4Request) throws NoBackendAvailableException {
        Backend backend = sessionPersistence.getBackend(l4Request);
        if (backend != null) {
            return new L4Response(backend);
        }

        // Get Number Of Maximum Connection on a Backend
        int currentMaxConnections = cluster.stream()
                .mapToInt(Backend::getActiveConnections)
                .max()
                .getAsInt();

        // Check If we got any Backend which has less Number of Connections than Backend with Maximum Connection
        Optional<Backend> optionalBackend = cluster.stream()
                .filter(back -> back.getActiveConnections() < currentMaxConnections)
                .findFirst();

        backend = optionalBackend.orElseGet(() -> cluster.next());
        if (backend == null) {
            throw new NoBackendAvailableException("No Backend available for Cluster: " + cluster);
        }
        sessionPersistence.addRoute(l4Request.getSocketAddress(), backend);
        return new L4Response(backend);
    }
}
