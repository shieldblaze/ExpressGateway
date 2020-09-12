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
package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.atomic.AtomicInteger;

public final class LeastConnection extends L4Balance {

    private final AtomicInteger Index = new AtomicInteger();

    public LeastConnection(List<Backend> backends) {
        super(backends);
        if (backends.size() == 0) {
            throw new IllegalArgumentException("Backends List cannot be empty");
        }
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        if (Index.get() >= backends.size()) {
            Index.set(0);
        }

        int currentMaxConnections = backends.stream()
                .mapToInt(Backend::getConnections)
                .max()
                .getAsInt();

        Optional<Backend> backend = backends.stream()
                .filter(back -> back.getConnections() < currentMaxConnections)
                .findFirst();

        return backend.orElseGet(() -> backends.get(Index.getAndIncrement()));
    }
}
