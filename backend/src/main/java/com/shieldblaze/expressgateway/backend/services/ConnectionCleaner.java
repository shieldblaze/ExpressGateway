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
package com.shieldblaze.expressgateway.backend.services;

import com.shieldblaze.expressgateway.backend.Node;

final class ConnectionCleaner implements Runnable {

    private final Node node;

    ConnectionCleaner(Node node) {
        this.node = node;
    }

    @Override
    public void run() {
        // Remove connection from queue if they're not active.
        node.availableConnections().removeIf(connection -> {
            // If Connection has timed out connecting limit and connection is not active
            // then we can safely remove connection.
            if (connection.hasConnectionTimedOut() && !connection.isActive()) {
                node.removeConnection(connection);
                return true;
            } else {
                return false;
            }
        });

        // Remove connection from queue if they're not active.
        node.activeConnections().removeIf(connection -> {
            // If Connection has timed out connecting limit and connection is not active
            // then we can safely remove connection.
            if (connection.hasConnectionTimedOut() && !connection.isActive()) {
                node.removeConnection(connection);
                return true;
            } else {
                return false;
            }
        });
    }
}
