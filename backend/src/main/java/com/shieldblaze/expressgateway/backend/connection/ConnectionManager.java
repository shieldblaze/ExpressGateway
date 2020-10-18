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
import com.shieldblaze.expressgateway.backend.BackendNotAvailableException;
import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;

import java.util.Objects;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Connection Manager is a connection pool for {@link Backend}.
 */
public abstract class ConnectionManager {

    final ConcurrentLinkedQueue<Connection> activeConnections = new ConcurrentLinkedQueue<>();
    final ConcurrentLinkedQueue<Connection> availableConnections = new ConcurrentLinkedQueue<>();

    private final ScheduledFuture<?> scheduledFutureConnectionLifecycleManager;

    protected Backend backend;
    protected final Bootstrapper bootstrapper;

    /**
     * Create a new {@link ConnectionManager} Instance
     *
     * @param bootstrapper {@link Bootstrapper} Implementation
     */
    public ConnectionManager(Bootstrapper bootstrapper) {
        this.bootstrapper = Objects.requireNonNull(bootstrapper, "bootstrapper");
        scheduledFutureConnectionLifecycleManager = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(
                new ConnectionLifecycleManager(this), 100, 100, TimeUnit.MILLISECONDS
        );
    }

    /**
     * Acquire a existing available connection or create a new connection.
     *
     * @return {@link Connection} Instance
     * @throws TooManyConnectionsException If maximum number of connections has reached
     */
    public abstract Connection acquireConnection() throws BackendNotAvailableException;

    /**
     * Drain all active connections and shutdown {@link ConnectionManager}
     */
    public void drainAllAndShutdown() {
        scheduledFutureConnectionLifecycleManager.cancel(true);
        activeConnections.forEach(connection -> connection.channelFuture.channel().close());
        activeConnections.clear();
        availableConnections.clear();
    }

    /**
     * Set {@link Backend}
     * @param backend {@link Backend} Instance
     */
    public void setBackend(Backend backend) {
        if (this.backend == null) {
            this.backend = Objects.requireNonNull(backend, "backend");
        }
    }
}
