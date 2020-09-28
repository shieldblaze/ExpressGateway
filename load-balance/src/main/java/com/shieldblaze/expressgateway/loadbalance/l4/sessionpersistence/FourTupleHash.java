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

import com.google.common.cache.Cache;
import com.google.common.cache.CacheBuilder;
import com.shieldblaze.expressgateway.loadbalance.backend.Backend;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;

/**
 * <p> 4-Tuple Hash based {@link SessionPersistence} </p>
 * <p> Source IP Address + Source Port + Destination IP Address + Destination Port </p>
 */
public final class FourTupleHash extends SessionPersistence {

    private final Cache<InetSocketAddress, Backend> routeCache = CacheBuilder.newBuilder()
            .maximumSize(1_000_000)
            .expireAfterWrite(1, TimeUnit.HOURS)
            .build();

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        return routeCache.getIfPresent(sourceAddress);
    }

    @Override
    public void addRoute(InetSocketAddress socketAddress, Backend backend) {
        routeCache.put(socketAddress, backend);
    }
}
