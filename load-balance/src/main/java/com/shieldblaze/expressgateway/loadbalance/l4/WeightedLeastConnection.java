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
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.loadbalance.exceptions.NoBackendAvailableException;
import com.shieldblaze.expressgateway.loadbalance.l4.sessionpersistence.NOOPSessionPersistence;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;

/**
 * Select {@link Backend} Based on Weight with Least Connection using Round-Robin
 */
public final class WeightedLeastConnection extends L4Balance implements EventListener {

    private final List<Backend> backends = new ArrayList<>();

    public WeightedLeastConnection() {
        super(new NOOPSessionPersistence());
    }

    public WeightedLeastConnection(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public WeightedLeastConnection(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, Cluster cluster) {
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
        sessionPersistence.clear();
        backends.clear();

        backends.addAll(cluster.onlineBackends());
    }

    @Override
    public L4Response response(L4Request l4Request) throws LoadBalanceException {
        Backend backend = sessionPersistence.backend(l4Request);
        if (backend != null) {
            // If Backend is ONLINE then return the response
            // else remove it from session persistence.
            if (backend.state() == State.ONLINE) {
                return new L4Response(backend);
            } else {
                sessionPersistence.removeRoute(l4Request.socketAddress(), backend);
            }
        }

        Optional<Backend> optionalBackend = backends.stream()
                .reduce((a, b) -> a.load() < b.load() ? a : b);

        if (optionalBackend.isPresent()) {
            backend = optionalBackend.get();
        } else {
            throw new NoBackendAvailableException();
        }
;
        sessionPersistence.addRoute(l4Request.socketAddress(), backend);
        return new L4Response(backend);
    }

    @Override
    public void accept(Object event) {
        if (event instanceof BackendEvent) {
            BackendEvent backendEvent = (BackendEvent) event;
            switch (backendEvent.type()) {
                case ADDED:
                case ONLINE:
                case OFFLINE:
                case REMOVED:
                   init();
                default:
                    throw new IllegalArgumentException("Unsupported Backend Event Type: " + backendEvent.type());
            }
        }
    }
}
