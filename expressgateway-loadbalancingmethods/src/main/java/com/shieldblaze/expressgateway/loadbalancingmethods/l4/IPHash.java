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

import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

public final class IPHash extends L4Balancer {

    private final RoundRobinListImpl<InetSocketAddress> backendAddressesRoundRobin;

    /**
     * {@link InetAddress} Client Address
     * {@link InetSocketAddress} Linked Backend Address
     */
    private final Map<InetAddress, InetSocketAddress> ipHashMap = new HashMap<>();

    public IPHash(List<InetSocketAddress> socketAddressList) {
        super(socketAddressList);
        backendAddressesRoundRobin = new RoundRobinListImpl<>(backendAddresses);
    }

    @Override
    public InetSocketAddress getBackendAddress(InetSocketAddress sourceAddress) {
        InetSocketAddress backendAddress = ipHashMap.get(sourceAddress.getAddress());

        if (backendAddress == null) {
            backendAddress = backendAddressesRoundRobin.iterator().next();
            ipHashMap.put(sourceAddress.getAddress(), backendAddress);
        }

        return backendAddress;
    }
}
