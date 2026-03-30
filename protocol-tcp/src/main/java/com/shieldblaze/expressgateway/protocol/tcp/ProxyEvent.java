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

import java.net.InetSocketAddress;
import java.time.Instant;

/**
 * Sealed interface for TCP proxy lifecycle events.
 *
 * <p>Each event captures the precise moment and context of a state transition
 * in the proxy session. The sealed hierarchy allows exhaustive pattern matching
 * in event consumers with compile-time safety -- adding a new event type forces
 * all switch expressions to be updated.</p>
 *
 * <p>All implementations are records for immutability and zero-overhead construction.</p>
 */
sealed interface ProxyEvent {

    Instant timestamp();

    /**
     * A new client connected to the proxy frontend.
     *
     * @param timestamp    when the connection was accepted
     * @param clientAddress the client's remote address
     */
    record ClientConnected(
            Instant timestamp,
            InetSocketAddress clientAddress
    ) implements ProxyEvent {}

    /**
     * Backend connection established successfully.
     *
     * @param timestamp      when the backend channel became active
     * @param clientAddress  the client's remote address
     * @param backendAddress the backend node address
     * @param connectTimeMs  time taken to establish the backend connection in milliseconds
     */
    record BackendConnected(
            Instant timestamp,
            InetSocketAddress clientAddress,
            InetSocketAddress backendAddress,
            long connectTimeMs
    ) implements ProxyEvent {}

    /**
     * Backend connection failed.
     *
     * @param timestamp      when the failure was detected
     * @param clientAddress  the client's remote address
     * @param backendAddress the backend node address
     * @param cause          the failure cause
     */
    record BackendConnectFailed(
            Instant timestamp,
            InetSocketAddress clientAddress,
            InetSocketAddress backendAddress,
            Throwable cause
    ) implements ProxyEvent {}

    /**
     * Backpressure engaged -- autoRead toggled off on one side.
     *
     * @param timestamp when backpressure was engaged
     * @param direction which direction is paused
     */
    record BackpressureEngaged(
            Instant timestamp,
            Direction direction
    ) implements ProxyEvent {}

    /**
     * Backpressure released -- autoRead toggled back on.
     *
     * @param timestamp when backpressure was released
     * @param direction which direction was resumed
     */
    record BackpressureReleased(
            Instant timestamp,
            Direction direction
    ) implements ProxyEvent {}

    /**
     * Half-close detected on one side of the connection (RFC 9293 Section 3.6).
     *
     * @param timestamp when the FIN was received
     * @param direction which side sent the FIN
     */
    record HalfClose(
            Instant timestamp,
            Direction direction
    ) implements ProxyEvent {}

    /**
     * Proxy session fully closed.
     *
     * @param timestamp  when the session was closed
     * @param statistics final connection statistics snapshot
     * @param reason     why the connection was closed
     */
    record SessionClosed(
            Instant timestamp,
            ConnectionStatistics.Snapshot statistics,
            CloseReason reason
    ) implements ProxyEvent {}

    /**
     * Direction of data flow in the proxy.
     */
    enum Direction {
        /** Client to backend (upstream to downstream) */
        CLIENT_TO_BACKEND,
        /** Backend to client (downstream to upstream) */
        BACKEND_TO_CLIENT
    }

    /**
     * Reason for connection closure.
     */
    enum CloseReason {
        CLIENT_CLOSED,
        BACKEND_CLOSED,
        IDLE_TIMEOUT,
        BACKEND_CONNECT_FAILURE,
        ERROR,
        DRAIN_SHUTDOWN,
        NO_BACKEND_AVAILABLE
    }
}
