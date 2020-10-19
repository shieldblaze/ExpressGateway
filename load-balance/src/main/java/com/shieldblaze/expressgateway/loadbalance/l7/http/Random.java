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
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.loadbalance.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.loadbalance.exceptions.NoBackendAvailableException;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Response;

/**
 * Select {@link Backend} Randomly
 */
public final class Random extends HTTPBalance {
    private final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    public Random() {
        super(new NOOPSessionPersistence());
    }

    public Random(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public Random(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Backend> sessionPersistence, Cluster cluster) {
        super(sessionPersistence);
        super.setCluster(cluster);
    }

    @Override
    public HTTPBalanceResponse getResponse(HTTPBalanceRequest httpBalanceRequest) throws NoBackendAvailableException {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.getBackend(httpBalanceRequest);
        if (httpBalanceResponse != null) {
            // If Backend is ONLINE then return the response
            // else remove it from session persistence.
            if (httpBalanceResponse.getBackend().getState() == State.ONLINE) {
                return httpBalanceResponse;
            } else {
                sessionPersistence.removeRoute(httpBalanceRequest, httpBalanceResponse.getBackend());
            }
        }

        int index = RANDOM_INSTANCE.nextInt(cluster.online());

        Backend backend;
        try {
            backend = cluster.getOnline(index);
        } catch (BackendNotOnlineException e) {
            // If selected Backend is not online then
            // we'll throw an exception. However, this should
            // rarely or never happen in most cases.
            throw new NoBackendAvailableException("Randomly selected Backend is not online");
        }

        // Add to session persistence and return
        return sessionPersistence.addRoute(httpBalanceRequest, backend);
    }
}
