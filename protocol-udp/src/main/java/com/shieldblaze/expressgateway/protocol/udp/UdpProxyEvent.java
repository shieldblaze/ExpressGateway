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
 * Sealed interface for UDP proxy lifecycle events.
 *
 * <p>Each event captures the precise moment and context of a state transition
 * in a UDP proxy session. The sealed hierarchy allows exhaustive pattern matching
 * in event consumers with compile-time safety.</p>
 *
 * <p>All implementations are records for immutability and zero-overhead construction.</p>
 */
sealed interface UdpProxyEvent {

    Instant timestamp();

    /**
     * A new UDP session was created for a client.
     *
     * @param timestamp      when the session was created
     * @param clientAddress  the client's source address
     * @param backendAddress the backend node address
     */
    record SessionCreated(
            Instant timestamp,
            InetSocketAddress clientAddress,
            InetSocketAddress backendAddress
    ) implements UdpProxyEvent {}

    /**
     * A UDP session expired due to idle timeout.
     *
     * @param timestamp      when the session expired
     * @param clientAddress  the client's source address
     * @param backendAddress the backend node address
     * @param packetCount    total packets forwarded before expiry
     */
    record SessionExpired(
            Instant timestamp,
            InetSocketAddress clientAddress,
            InetSocketAddress backendAddress,
            long packetCount
    ) implements UdpProxyEvent {}

    /**
     * A datagram was dropped due to rate limiting.
     *
     * @param timestamp     when the drop occurred
     * @param clientAddress the client whose datagram was dropped
     * @param reason        why the datagram was dropped
     */
    record DatagramDropped(
            Instant timestamp,
            InetSocketAddress clientAddress,
            DropReason reason
    ) implements UdpProxyEvent {}

    /**
     * A datagram could not be delivered to the backend.
     *
     * @param timestamp     when the failure occurred
     * @param clientAddress the client's source address
     * @param cause         the failure cause
     */
    record DeliveryFailed(
            Instant timestamp,
            InetSocketAddress clientAddress,
            Throwable cause
    ) implements UdpProxyEvent {}

    /**
     * Reason for datagram drop.
     */
    enum DropReason {
        /** Per-source rate limit exceeded */
        RATE_LIMITED,
        /** No backend node available */
        NO_BACKEND,
        /** Datagram exceeds configured MTU */
        OVERSIZED,
        /** Backend connection not yet established */
        NOT_READY
    }
}
