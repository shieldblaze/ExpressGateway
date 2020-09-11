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

import io.netty.util.NetUtil;

import java.math.BigInteger;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Select Backend Based on Source IP Hash and Round-Robin
 */
public final class SourceIPHash extends L4Balancer {

    private static final BigInteger MINUS_ONE = BigInteger.valueOf(-1);

    private final RoundRobinListImpl<InetSocketAddress> backendAddressesRoundRobin;

    /**
     * {@link Object} {@link Integer} in case of IPv4 or {@link Byte) in case of IPv6 for Source Address
     * {@link InetSocketAddress} Linked Backend Address
     */
    private final Map<Object, InetSocketAddress> ipHashMap = new HashMap<>();

    public SourceIPHash(List<InetSocketAddress> socketAddressList) {
        super(socketAddressList);
        backendAddressesRoundRobin = new RoundRobinListImpl<>(getBackendAddresses());
    }

    @Override
    public InetSocketAddress getBackendAddress(InetSocketAddress sourceAddress) {

        /*
         * If Source IP Address is IPv4, we'll convert it into Integer with /24 mask.
         *
         * If Source IP Address is IPv6, we'll convert it into BigInteger with /48 mask.
         */
        if (sourceAddress.getAddress() instanceof Inet4Address) {
            int ipAddress = NetUtil.ipv4AddressToInt((Inet4Address) sourceAddress.getAddress());
            int ipWithMask = ipAddress & prefixToSubnetMaskIPv4();

            InetSocketAddress backendAddress = ipHashMap.get(ipWithMask);

            if (backendAddress == null) {
                backendAddress = backendAddressesRoundRobin.iterator().next();
                ipHashMap.put(ipWithMask, backendAddress);
            }

            return backendAddress;
        } else {
            BigInteger ipAddress = ipToInt((Inet6Address) sourceAddress.getAddress());
            BigInteger ipWithMask = ipAddress.and(prefixToSubnetMaskIPv6());

            InetSocketAddress backendAddress = ipHashMap.get(ipWithMask);

            if (backendAddress == null) {
                backendAddress = backendAddressesRoundRobin.iterator().next();
                ipHashMap.put(ipWithMask, backendAddress);
            }

            return backendAddress;
        }
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
