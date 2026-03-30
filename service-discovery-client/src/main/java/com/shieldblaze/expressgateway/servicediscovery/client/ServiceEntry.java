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
package com.shieldblaze.expressgateway.servicediscovery.client;

import java.time.Instant;

/**
 * Immutable record representing a service instance entry returned from service discovery.
 *
 * @param id         the unique instance ID
 * @param ipAddress  the IP address of the service
 * @param port       the port number
 * @param tlsEnabled whether TLS is enabled on this instance
 * @param healthy    whether this instance passed its last health check
 * @param fetchedAt  the instant this entry was fetched from the discovery server
 */
public record ServiceEntry(
        String id,
        String ipAddress,
        int port,
        boolean tlsEnabled,
        boolean healthy,
        Instant fetchedAt
) {

    /**
     * Create a service entry with the current time as fetchedAt and healthy=true.
     */
    public static ServiceEntry of(String id, String ipAddress, int port, boolean tlsEnabled) {
        return new ServiceEntry(id, ipAddress, port, tlsEnabled, true, Instant.now());
    }

    /**
     * Return the socket address string (ip:port).
     */
    public String address() {
        return ipAddress + ":" + port;
    }

    /**
     * Check if this entry is still fresh given the TTL in milliseconds.
     */
    public boolean isFresh(long ttlMillis) {
        return Instant.now().toEpochMilli() - fetchedAt.toEpochMilli() < ttlMillis;
    }
}
