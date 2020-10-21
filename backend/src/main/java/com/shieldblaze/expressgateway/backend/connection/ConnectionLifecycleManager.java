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

import java.util.Map;
import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * Connection Lifecycle Manager removes in-active {@link Connection}
 * from {@link ClusterConnectionPool#backendActiveConnectionMap} and adds
 * active and available to use {@link Connection} in {@link ClusterConnectionPool#backendAvailableConnectionMap}
 */
final class ConnectionLifecycleManager implements Runnable {

    private final ClusterConnectionPool connectionManager;

    /**
     * Create a new {@link ConnectionLifecycleManager} Instance
     *
     * @param connectionManager {@link ClusterConnectionPool} which we have to manage
     */
    ConnectionLifecycleManager(ClusterConnectionPool connectionManager) {
        this.connectionManager = connectionManager;
    }

    @Override
    public void run() {
        // Remove connections which are not active.
        for (Map.Entry<Backend, ConcurrentLinkedQueue<Connection>> entry : connectionManager.backendActiveConnectionMap.entrySet()) {
            entry.getValue().forEach(connection -> {
                if (!connection.isActive()) {
                    entry.getValue().remove(connection);
                    entry.getKey().decConnections();
                }
            });
        }

        // If connection is active and not in use then add it to available connections.
        for (Map.Entry<Backend, ConcurrentLinkedQueue<Connection>> entry : connectionManager.backendActiveConnectionMap.entrySet()) {
            entry.getValue().forEach(connection -> {
                if (connection.isActive() && !connection.isInUse()) {
                    connectionManager.backendAvailableConnectionMap.get(entry.getKey()).add(connection);
                }
            });
        }
    }
}
