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
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;
import java.util.List;

/**
 * Select {@link Backend} Randomly
 */
public final class Random extends L4Balance {
    private final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    public Random() {
        super(new NOOPSessionPersistence());
    }

    public Random(List<Backend> backends) {
        super(new NOOPSessionPersistence());
        setBackends(backends);
    }

    public Random(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, List<Backend> backends) {
        super(sessionPersistence);
        setBackends(backends);
    }

    @Override
    public L4Response getResponse(L4Request l4Request) {
        Backend backend = sessionPersistence.getBackend(new L4Request(l4Request.getSocketAddress()));
        if (backend != null) {
            return new L4Response(backend);
        }

        int index = RANDOM_INSTANCE.nextInt(backends.size());

        backend = backends.get(index);
        sessionPersistence.addRoute(l4Request.getSocketAddress(), backend);
        return new L4Response(backend);
    }
}
