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
package com.shieldblaze.expressgateway.protocol.tcp;

import io.netty.channel.Channel;

import java.net.InetSocketAddress;
import java.time.Instant;

/**
 * Immutable record representing a pooled backend TCP connection.
 *
 * <p>Each entry tracks the Netty channel, the backend address it connects to,
 * and when it was created. The channel's liveness is checked via {@link #isUsable()}
 * before returning it from the pool.</p>
 *
 * @param channel        the Netty channel to the backend
 * @param backendAddress the backend socket address this channel is connected to
 * @param createdAt      when this connection was established
 * @param lastUsedAt     when this connection was last used (for idle eviction)
 */
record ConnectionPoolEntry(
        Channel channel,
        InetSocketAddress backendAddress,
        Instant createdAt,
        Instant lastUsedAt
) {
    /**
     * Create a new entry with the current time as both creation and last-used time.
     */
    ConnectionPoolEntry(Channel channel, InetSocketAddress backendAddress) {
        this(channel, backendAddress, Instant.now(), Instant.now());
    }

    /**
     * Return a new entry with updated lastUsedAt timestamp (records are immutable).
     */
    ConnectionPoolEntry touch() {
        return new ConnectionPoolEntry(channel, backendAddress, createdAt, Instant.now());
    }

    /**
     * Check if this pooled connection is still usable.
     * A connection is usable if:
     * - The channel is not null
     * - The channel is active (TCP connection is alive)
     * - The channel is open
     */
    boolean isUsable() {
        return channel != null && channel.isActive() && channel.isOpen();
    }

    /**
     * Check if this entry has been idle longer than the given duration.
     */
    boolean isIdleLongerThan(java.time.Duration maxIdle) {
        return java.time.Duration.between(lastUsedAt, Instant.now()).compareTo(maxIdle) > 0;
    }
}
