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

import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.SessionPersistence;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Select {@link Backend} with least connections with Round-Robin.
 */
public final class LeastConnection extends L4Balance {

    private final AtomicInteger Index = new AtomicInteger();

    public LeastConnection() {
        this(new NOOPSessionPersistence());
    }

    public LeastConnection(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends);
    }

    public LeastConnection(SessionPersistence sessionPersistence) {
        super(sessionPersistence);
    }

    public LeastConnection(SessionPersistence sessionPersistence, List<Backend> backends) {
        super(sessionPersistence);
        setBackends(backends);
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        Backend backend = sessionPersistence.getBackend(sourceAddress);
        if (backend != null) {
            return backend;
        }

        // If Index size equals Backend List size, we'll reset the Index.
        if (Index.get() >= backends.size()) {
            Index.set(0);
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

        backend = optionalBackend.orElseGet(() -> backends.get(Index.getAndIncrement()));
        sessionPersistence.addRoute(sourceAddress, backend);
        return backend;
    }
}
