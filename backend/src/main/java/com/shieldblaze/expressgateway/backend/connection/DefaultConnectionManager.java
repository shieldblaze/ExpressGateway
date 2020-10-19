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
package com.shieldblaze.expressgateway.backend.connection;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotAvailableException;
import com.shieldblaze.expressgateway.backend.State;

/**
 * This is the default implementation of {@linkplain ConnectionManager}.
 * It only returns a {@linkplain Connection} on {@linkplain #acquireConnection()} call when {@linkplain Backend}
 * is {@linkplain State#ONLINE} and active connections does not exceed maximum connection.
 */
public final class DefaultConnectionManager extends ConnectionManager {

    /**
     * Create a new {@link ConnectionManager} Instance
     *
     * @param bootstrapper {@link Bootstrapper} Implementation
     */
    public DefaultConnectionManager(Bootstrapper bootstrapper) {
        super(bootstrapper);
    }

    @Override
    public Connection acquireConnection() throws BackendNotAvailableException {
        if (backend.getState() != State.ONLINE) {
            throw new BackendNotAvailableException("Backend: " + backend.getSocketAddress() + " is not online.");
        }

        Connection connection = availableConnections.poll();

        // If connection is available, return it.
        if (connection != null) {
            return connection;
        }

        // Check for connection limit before creating new connection.
        if (backend.getMaxConnections() > activeConnections.size()) {
            // Throw exception because we have too many active connections
            throw new TooManyConnectionsException(backend);
        }

        // Create a new connection
        connection = new Connection();
        activeConnections.add(connection);
        connection.channelFuture = bootstrapper.bootstrap().connect(backend.getSocketAddress());
        return connection;
    }
}
