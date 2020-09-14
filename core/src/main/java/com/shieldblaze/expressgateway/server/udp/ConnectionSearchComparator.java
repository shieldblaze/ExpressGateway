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
package com.shieldblaze.expressgateway.server.udp;

import com.google.common.primitives.SignedBytes;

import java.net.InetSocketAddress;
import java.util.Comparator;

final class ConnectionSearchComparator implements Comparator<Object> {

    static final ConnectionSearchComparator INSTANCE = new ConnectionSearchComparator();

    private ConnectionSearchComparator() {
        // Prevent outside initialization
    }

    @Override
    public int compare(Object o1, Object o2) {
        Connection connection = (Connection) o1;
        InetSocketAddress socketAddress = (InetSocketAddress) o2;

        int compare = SignedBytes.lexicographicalComparator().compare(connection.clientAddress.getAddress().getAddress(),
                socketAddress.getAddress().getAddress());

        if (compare == 0) {
            return Integer.compare(connection.clientAddress.getPort(), socketAddress.getPort());
        } else {
            return compare;
        }
    }
}
