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
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotAvailableException;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;
import io.netty.channel.ChannelHandler;

import java.util.Objects;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

public abstract class ClusterConnectionPool {

    protected final ExtendingConcurrentSkipListMap backendActiveConnectionMap = new ExtendingConcurrentSkipListMap();
    protected final ExtendingConcurrentSkipListMap backendAvailableConnectionMap = new ExtendingConcurrentSkipListMap();

    /**
     * {@link Bootstrapper} Implementation Instance
     */
    protected final Bootstrapper bootstrapper;

    /**
     * {@link ScheduledFuture} of {@link ConnectionLifecycleManager}
     */
    private final ScheduledFuture<?> scheduledFutureConnectionLifecycleManager;

    /**
     * Cluster associated with this {@linkplain ClusterConnectionPool}
     */
    private Cluster cluster;

    /**
     * Load Balance to use for load-balancing
     */
    private LoadBalance<?, ?, ?, ?> loadBalance;

    public ClusterConnectionPool(Bootstrapper bootstrapper) {
        this.bootstrapper = Objects.requireNonNull(bootstrapper, "Bootstrapper");
        scheduledFutureConnectionLifecycleManager = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(
                new ConnectionLifecycleManager(this), 100, 100, TimeUnit.MILLISECONDS
        );
    }

    public Response getResponse(Request request) throws LoadBalanceException {
        return loadBalance.getResponse(request);
    }

    public abstract Connection acquireConnection(Response response, ChannelHandler downstreamHandler) throws BackendNotAvailableException;

    public void setCluster(Cluster cluster) {
        if (this.cluster == null) {
            this.cluster = Objects.requireNonNull(cluster, "Cluster");
        } else {
            throw new IllegalArgumentException("Cluster is already set");
        }
    }

    public void setLoadBalance(LoadBalance<?, ?, ?, ?> loadBalance) {
        if (this.loadBalance == null) {
            this.loadBalance = Objects.requireNonNull(loadBalance, "LoadBalance");
        } else {
            throw new IllegalArgumentException("LoadBalance is already set");
        }
    }

    /**
     * Drain all connections of a {@linkplain Backend}
     */
    public void drainAllConnection(Backend backend) {
        backendActiveConnectionMap.get(backend).forEach(connection -> connection.channelFuture.channel().close());
        backendActiveConnectionMap.get(backend).clear();
        backendAvailableConnectionMap.get(backend).clear();
    }

    /**
     * Drain all connections of a {@linkplain Cluster} and {@linkplain ClusterConnectionPool}
     */
    public void drainAllConnectionAndShutdown() {
        scheduledFutureConnectionLifecycleManager.cancel(true);
        backendActiveConnectionMap.forEach((backend, connections) -> connections.forEach(connection -> connection.channelFuture.channel().close()));
        backendActiveConnectionMap.clear();
        backendAvailableConnectionMap.clear();
    }
}
