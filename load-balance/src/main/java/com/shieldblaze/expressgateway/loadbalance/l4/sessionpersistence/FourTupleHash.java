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
package com.shieldblaze.expressgateway.loadbalance.l4.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.common.map.SelfExpiringMap;
import com.shieldblaze.expressgateway.common.utils.comparator.InetSocketAddressHashCodeComparator;
import com.shieldblaze.expressgateway.loadbalance.Request;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.concurrent.ConcurrentSkipListMap;

/**
 * <p> 4-Tuple Hash based {@link SessionPersistence} </p>
 * <p> Source IP Address + Source Port + Destination IP Address + Destination Port </p>
 */
public final class FourTupleHash implements SessionPersistence<Backend, Backend, InetSocketAddress, Backend> {

    private final SelfExpiringMap<InetSocketAddress, Backend> routeMap = new SelfExpiringMap<>(
            new ConcurrentSkipListMap<>(InetSocketAddressHashCodeComparator.INSTANCE), Duration.ofHours(1), false);

    @Override
    public Backend getBackend(Request request) {
        return routeMap.get(request.getSocketAddress());
    }

    @Override
    public Backend addRoute(InetSocketAddress socketAddress, Backend backend) {
        routeMap.put(socketAddress, backend);
        return backend;
    }

    @Override
    public boolean removeRoute(InetSocketAddress inetSocketAddress, Backend backend) {
        return routeMap.remove(inetSocketAddress, backend);
    }

    @Override
    public void clear() {
        routeMap.clear();
    }
}
