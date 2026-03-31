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
package com.shieldblaze.expressgateway.protocol.udp;

import java.net.InetSocketAddress;
import java.time.Instant;

/**
 * Immutable record representing a tracked UDP session.
 *
 * <p>UDP is connectionless, so "sessions" are synthetic mappings from client
 * source address to the upstream connection used to proxy their datagrams.
 * This record captures the metadata for such a session.</p>
 *
 * @param clientAddress   the client's source address (session key)
 * @param backendAddress  the backend node this session is mapped to
 * @param connection      the upstream connection to the backend
 * @param createdAt       when the session was first established
 * @param packetCount     total packets forwarded in this session
 * @param byteCount       total bytes forwarded in this session
 */
record UdpSessionEntry(
        InetSocketAddress clientAddress,
        InetSocketAddress backendAddress,
        UDPConnection connection,
        Instant createdAt,
        long packetCount,
        long byteCount
) {
    /**
     * Create a new session entry with initial counts.
     */
    UdpSessionEntry(InetSocketAddress clientAddress, InetSocketAddress backendAddress, UDPConnection connection) {
        this(clientAddress, backendAddress, connection, Instant.now(), 0, 0);
    }

    /**
     * Check if this session is still active (backend channel is alive).
     */
    boolean isActive() {
        return connection != null
                && connection.channel() != null
                && connection.channel().isActive();
    }
}
