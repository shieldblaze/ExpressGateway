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
import com.shieldblaze.expressgateway.common.list.RoundRobinList;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;
import io.netty.handler.codec.http.EmptyHttpHeaders;

import java.util.List;

public final class PerRequestRoundRobin extends HTTPL7Balance {

    private final boolean enableHTTP2;
    private RoundRobinList<Backend> backendsRoundRobin;

    public PerRequestRoundRobin() {
        super(new NOOPSessionPersistence());
        enableHTTP2 = true;
    }

    public PerRequestRoundRobin(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends, true);
    }

    public PerRequestRoundRobin(SessionPersistence<Backend, HTTPRequest, HTTPResponse> sessionPersistence, List<Backend> backends, boolean enableHTTP2) {
        super(sessionPersistence);
        setBackends(backends);
        this.enableHTTP2 = enableHTTP2;
    }

    @Override
    public void setBackends(List<Backend> backends) {
        super.setBackends(backends);
        backendsRoundRobin = new RoundRobinList<>(this.backends);
    }

    @Override
    public HTTPResponse getBackend(HTTPRequest httpRequest) {
        Backend backend = sessionPersistence.getBackend(httpRequest);
        if (backend != null) {
            return new HTTPResponse(backend, EmptyHttpHeaders.INSTANCE);
        }

        backend = backendsRoundRobin.iterator().next();

        HTTPResponse httpResponse = new HTTPResponse(backend, EmptyHttpHeaders.INSTANCE);
        sessionPersistence.addRoute(httpRequest, httpResponse);
        return httpResponse;
    }
}
