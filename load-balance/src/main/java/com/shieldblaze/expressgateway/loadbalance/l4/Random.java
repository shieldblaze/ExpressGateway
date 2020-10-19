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
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.loadbalance.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;

/**
 * Select {@link Backend} Randomly
 */
public final class Random extends L4Balance {
    private final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    public Random() {
        super(new NOOPSessionPersistence());
    }

    public Random(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public Random(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, Cluster cluster) {
        super(sessionPersistence);
        setCluster(cluster);
    }

    @Override
    public L4Response getResponse(L4Request l4Request) throws LoadBalanceException {
        Backend backend = sessionPersistence.getBackend(l4Request);
        if (backend != null) {
            return new L4Response(backend);
        }

        int index = RANDOM_INSTANCE.nextInt(cluster.online());

        try {
            backend = cluster.getOnline(index);
        } catch (BackendNotOnlineException e) {
            throw new LoadBalanceException("Randomly selected Backend is not online");
        }

        // Add to session persistence
        sessionPersistence.addRoute(l4Request.getSocketAddress(), backend);
        return new L4Response(backend);
    }
}
