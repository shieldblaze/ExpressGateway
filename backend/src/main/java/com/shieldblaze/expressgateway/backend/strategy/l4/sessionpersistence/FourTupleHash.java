/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.common.map.SelfExpiringMap;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * <p> 4-Tuple Hash based {@link SessionPersistence} </p>
 * <p> Source IP Address + Source Port + Destination IP Address + Destination Port </p>
 */
public final class FourTupleHash implements SessionPersistence<Node, Node, InetSocketAddress, Node> {

    private final Map<InetSocketAddress, Node> routeMap =
            new SelfExpiringMap<>(new ConcurrentHashMap<>(), Duration.ofHours(1), false);

    @Override
    public Node node(Request request) {
        return routeMap.get(request.socketAddress());
    }

    @Override
    public Node addRoute(InetSocketAddress socketAddress, Node node) {
        routeMap.put(socketAddress, node);
        return node;
    }

    @Override
    public boolean removeRoute(InetSocketAddress inetSocketAddress, Node node) {
        return routeMap.remove(inetSocketAddress, node);
    }

    @Override
    public boolean remove(Node nodeToRemove) {
        AtomicBoolean isRemoved = new AtomicBoolean(false);

        routeMap.forEach((socketAddress, node) -> {
            if (node == nodeToRemove) {
                routeMap.remove(socketAddress, node);
                isRemoved.set(true);
            }
        });

        return isRemoved.get();
    }

    @Override
    public void clear() {
        routeMap.clear();
    }

    @Override
    public String toString() {
        return "FourTupleHash{" +
                "routeMap=" + routeMap +
                '}';
    }

    @Override
    public String name() {
        return "FourTupleHash";
    }
}
