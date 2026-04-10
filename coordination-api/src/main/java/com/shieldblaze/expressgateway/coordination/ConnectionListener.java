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
package com.shieldblaze.expressgateway.coordination;

/**
 * Listener for connection state transitions between this node and the
 * coordination backend.
 *
 * <p>Implementations MUST be thread-safe: callbacks may fire from any
 * backend-internal thread.</p>
 */
@FunctionalInterface
public interface ConnectionListener {

    /**
     * Called when the connection state to the coordination backend changes.
     *
     * @param state the new connection state
     */
    void onConnectionStateChange(ConnectionState state);

    /**
     * Connection states between this node and the coordination backend.
     *
     * <p>State transitions:
     * <pre>
     *   CONNECTED ----> SUSPENDED ----> LOST
     *       ^               |              |
     *       |               v              |
     *       +--------- RECONNECTED <-------+
     * </pre>
     * </p>
     */
    enum ConnectionState {
        /** Initial connection established or re-established after full loss. */
        CONNECTED,

        /** Connection temporarily interrupted; session may still be valid (ZK) or
         *  keepalive lease may still be active (etcd). Reads may be stale. */
        SUSPENDED,

        /** Connection fully lost; ephemeral nodes are gone, locks are invalid,
         *  leadership should be assumed lost. */
        LOST,

        /** Connection restored after a SUSPENDED or LOST state. Ephemeral state
         *  should be re-established by the caller. */
        RECONNECTED
    }
}
