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
package com.shieldblaze.expressgateway.loadbalance.l7;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.SessionPersistence;
import io.netty.handler.codec.http.HttpRequest;

import java.util.List;

/**
 * Select {@link Backend} based on Round-Robin
 */
public final class RoundRobin extends L7Balance {

    private RoundRobinImpl<Backend> backendsRoundRobin;

    public RoundRobin() {
        super(new NOOPSessionPersistence());
    }

    public RoundRobin(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends);
    }

    public RoundRobin(SessionPersistence sessionPersistence, List<Backend> backends) {
        super(sessionPersistence);
        setBackends(backends);
    }

    @Override
    public void setBackends(List<Backend> backends) {
        super.setBackends(backends);
        backendsRoundRobin = new RoundRobinImpl<>(this.backends);
    }

    @Override
    public Backend getBackend(HttpRequest httpRequest) {
        Backend backend = sessionPersistence.getBackend(httpRequest);
        if (backend != null) {
            return backend;
        }

        backend = backendsRoundRobin.iterator().next();
        sessionPersistence.addRoute(httpRequest, backend);
        return backend;
    }
}
