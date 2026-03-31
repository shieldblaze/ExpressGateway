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
package com.shieldblaze.expressgateway.servicediscovery.server;

import java.time.Instant;

/**
 * Internal registration entry that wraps a {@link Node} with metadata for TTL tracking,
 * health status, and last-heartbeat timestamp.
 *
 * @param node          the service node information
 * @param registeredAt  timestamp when this entry was registered
 * @param lastHeartbeat timestamp of the last health check heartbeat
 * @param healthy       current health status
 * @param ttlSeconds    time-to-live in seconds (0 = no expiry)
 */
public record RegistrationEntry(
        Node node,
        Instant registeredAt,
        Instant lastHeartbeat,
        boolean healthy,
        long ttlSeconds
) {

    /**
     * Create a new registration with the current time and healthy status.
     */
    public static RegistrationEntry create(Node node, long ttlSeconds) {
        Instant now = Instant.now();
        return new RegistrationEntry(node, now, now, true, ttlSeconds);
    }

    /**
     * Return a copy with updated heartbeat timestamp.
     */
    public RegistrationEntry withHeartbeat() {
        return new RegistrationEntry(node, registeredAt, Instant.now(), true, ttlSeconds);
    }

    /**
     * Return a copy marked as unhealthy.
     */
    public RegistrationEntry asUnhealthy() {
        return new RegistrationEntry(node, registeredAt, lastHeartbeat, false, ttlSeconds);
    }

    /**
     * Check if this registration has expired based on the TTL.
     * TTL of 0 means no expiry. Uses millisecond precision for accuracy.
     */
    public boolean isExpired() {
        if (ttlSeconds <= 0) {
            return false;
        }
        return Instant.now().toEpochMilli() - lastHeartbeat.toEpochMilli() > ttlSeconds * 1000L;
    }
}
