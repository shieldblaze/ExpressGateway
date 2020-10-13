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
import java.util.Optional;

/**
 * Select {@link Backend} with least connections with Round-Robin.
 */
public final class LeastConnection extends L4Balance {

    private int index;

    public LeastConnection() {
        this(new NOOPSessionPersistence());
    }

    public LeastConnection(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends);
    }

    public LeastConnection(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence) {
        super(sessionPersistence);
    }

    public LeastConnection(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, List<Backend> backends) {
        super(sessionPersistence);
        setBackends(backends);
    }

    @Override
    public L4Response getResponse(L4Request l4Request) {
        Backend backend = sessionPersistence.getBackend(l4Request);
        if (backend != null) {
            return new L4Response(backend);
        }

        // If Index size equals Backend List size, we'll reset the Index.
        if (index >= backends.size()) {
            index = 0;
        }

        // Get Number Of Maximum Connection on a Backend
        int currentMaxConnections = backends.stream()
                .mapToInt(Backend::getActiveConnections)
                .max()
                .getAsInt();

        // Check If we got any Backend which has less Number of Connections than Backend with Maximum Connection
        Optional<Backend> optionalBackend = backends.stream()
                .filter(back -> back.getActiveConnections() < currentMaxConnections)
                .findFirst();

        backend = optionalBackend.orElseGet(() -> backends.get(index++));
        sessionPersistence.addRoute(l4Request.getSocketAddress(), backend);
        return new L4Response(backend);
    }
}
