/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
import io.netty.util.NetUtil;

import java.math.BigInteger;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicBoolean;

public final class SourceIPHash implements SessionPersistence<Node, Node, InetSocketAddress, Node> {

    private static final BigInteger MINUS_ONE = BigInteger.valueOf(-1);

    private final SelfExpiringMap<Object, Node> routeMap = new SelfExpiringMap<>(new ConcurrentHashMap<>(), Duration.ofHours(1), false);

    @Override
    public Node node(Request request) {
        Node node;

        /*
         * If Source IP Address is IPv4, we'll convert it into Integer with /24 mask.
         *
         * If Source IP Address is IPv6, we'll convert it into BigInteger with /48 mask.
         */
        if (request.socketAddress().getAddress() instanceof Inet4Address) {
            int ipWithMask = ipv4WithMask(request);
            node = routeMap.get(ipWithMask);
        } else {
            BigInteger ipWithMask = ipv6WithMask(request);
            node = routeMap.get(ipWithMask);
        }

        return node;
    }

    @Override
    public Node addRoute(InetSocketAddress socketAddress, Node node) {
        Object key;
        if (socketAddress.getAddress() instanceof Inet4Address) {
            key = ipv4WithMask(socketAddress);
        } else {
            key = ipv6WithMask(socketAddress);
        }

        routeMap.put(key, node);
        return node;
    }

    @Override
    public boolean removeRoute(InetSocketAddress socketAddress, Node node) {
        Object key;
        if (socketAddress.getAddress() instanceof Inet4Address) {
            key = ipv4WithMask(socketAddress);
        } else {
            key = ipv6WithMask(socketAddress);
        }
        return routeMap.remove(key, node);
    }

    @Override
    public boolean remove(Node nodeToRemove) {
        AtomicBoolean isRemoved = new AtomicBoolean(false);

        routeMap.forEach((object, node) -> {
            if (node == nodeToRemove) {
                routeMap.remove(object, node);
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
    public String name() {
        return "SourceIPHash";
    }

    private int ipv4WithMask(Request request) {
        return ipv4WithMask(request.socketAddress());
    }

    private BigInteger ipv6WithMask(Request request) {
        return ipv6WithMask(request.socketAddress());
    }

    private int ipv4WithMask(InetSocketAddress socketAddress) {
        int ipAddress = NetUtil.ipv4AddressToInt((Inet4Address) socketAddress.getAddress());
        return ipAddress & prefixToSubnetMaskIPv4();
    }

    private BigInteger ipv6WithMask(InetSocketAddress socketAddress) {
        BigInteger ipAddress = ipToInt((Inet6Address) socketAddress.getAddress());
        return ipAddress.and(prefixToSubnetMaskIPv6());
    }

    private static BigInteger ipToInt(Inet6Address ipAddress) {
        return new BigInteger(ipAddress.getAddress());
    }

    private static int prefixToSubnetMaskIPv4() {
        return (int) (-1L << 32 - 24);
    }

    private static BigInteger prefixToSubnetMaskIPv6() {
        return MINUS_ONE.shiftLeft(128 - 48);
    }
}
