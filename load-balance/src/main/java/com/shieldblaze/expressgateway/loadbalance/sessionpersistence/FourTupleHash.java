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
package com.shieldblaze.expressgateway.loadbalance.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.common.SelfExpiringMap;
import com.shieldblaze.expressgateway.loadbalance.l7.Request;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.concurrent.ConcurrentHashMap;

/**
 * <p> 4-Tuple Hash based {@link SessionPersistence} </p>
 * <p> Source IP Address + Source Port + Destination IP Address + Destination Port </p>
 */
public final class FourTupleHash extends SessionPersistence {

    private final SelfExpiringMap<String, Backend> routeMap = new SelfExpiringMap<>(new ConcurrentHashMap<>(), Duration.ofHours(1), false);

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        return routeMap.get(sourceAddress.toString());
    }

    @Override
    public Backend getBackend(Request request) {
        return null;
    }

    @Override
    public void addRoute(InetSocketAddress socketAddress, Backend backend) {
        routeMap.put(socketAddress.toString(), backend);
    }

    @Override
    public void addRoute(Request request, Backend backend) {
        // Does nothing
    }
}
