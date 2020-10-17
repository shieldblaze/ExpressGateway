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
import com.shieldblaze.expressgateway.common.list.RoundRobinList;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;
import java.util.List;

/**
 * Select {@link Backend} based on Round-Robin
 */
public final class RoundRobin extends L4Balance {

    private RoundRobinList<Backend> backendsRoundRobin;

    public RoundRobin() {
        super(new NOOPSessionPersistence());
    }

    public RoundRobin(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends);
    }

    public RoundRobin(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, List<Backend> backends) {
        super(sessionPersistence);
        setBackends(backends);
    }

    @Override
    public void setBackends(List<Backend> backends) {
        super.setBackends(backends);
        backendsRoundRobin = new RoundRobinList<>(this.backends);
    }

    @Override
    public L4Response getResponse(L4Request l4Request) {
        Backend backend = sessionPersistence.getBackend(new L4Request(l4Request.getSocketAddress()));
        if (backend != null) {
            return new L4Response(backend);
        }

        backend = backendsRoundRobin.iterator().next();
        sessionPersistence.addRoute(l4Request.getSocketAddress(), backend);
        return new L4Response(backend);
    }
}
