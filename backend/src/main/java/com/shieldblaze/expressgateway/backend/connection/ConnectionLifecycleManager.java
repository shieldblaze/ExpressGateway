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

/**
 * Connection Lifecycle Manager removes in-active {@link Connection}
 * from {@link ConnectionManager#activeConnections} and adds
 * active and available to use {@link Connection} in {@link ConnectionManager#availableConnections}
 */
final class ConnectionLifecycleManager implements Runnable {

    private final ConnectionManager connectionManager;

    /**
     * Create a new {@link ConnectionLifecycleManager} Instance
     *
     * @param connectionManager {@link ConnectionManager} which we have to manage
     */
    ConnectionLifecycleManager(ConnectionManager connectionManager) {
        this.connectionManager = connectionManager;
    }

    @Override
    public void run() {
        // Remove connections which are not active.
        connectionManager.activeConnections.forEach(connection -> {
            if (!connection.isActive()) {
                connectionManager.activeConnections.remove(connection);
                connectionManager.backend.decConnections();
            }
        });

        // If connection is active and not in use then add it to available connections.
        connectionManager.activeConnections.forEach(connection -> {
            if (connection.isActive() && !connection.isInUse()) {
                connectionManager.availableConnections.add(connection);
            }
        });
    }
}
