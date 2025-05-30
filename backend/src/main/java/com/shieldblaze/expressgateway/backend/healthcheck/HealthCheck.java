/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2024 ShieldBlaze
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

package com.shieldblaze.expressgateway.backend.healthcheck;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.concurrent.task.SyncTask;

import java.io.Closeable;

/**
 * {@link HealthCheck} is a service which is responsible for checking the health of the nodes.
 */
public interface HealthCheck extends Closeable {

    /**
     * Add a node to the health check service.
     *
     * @param node Node to add
     * @return {@code true} if the node is added else {@code false}
     */
    SyncTask<Boolean> add(Node node);

    /**
     * Remove a node from the health check service.
     *
     * @param node Node to remove
     * @return {@code true} if the node is removed else {@code false}
     */
    SyncTask<Boolean> remove(Node node);
}
