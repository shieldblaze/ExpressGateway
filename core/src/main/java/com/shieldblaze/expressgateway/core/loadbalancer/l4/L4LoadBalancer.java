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
package com.shieldblaze.expressgateway.core.loadbalancer.l4;

import com.shieldblaze.expressgateway.backend.Cluster;
import com.shieldblaze.expressgateway.core.concurrent.async.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import com.shieldblaze.expressgateway.core.utils.EventLoopFactory;
import com.shieldblaze.expressgateway.core.utils.PooledByteBufAllocator;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Balance;
import io.netty.buffer.ByteBufAllocator;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;

/**
 * {@link L4LoadBalancer} holds base functions for a L4-Load Balancer.
 */
public abstract class L4LoadBalancer {

    private final InetSocketAddress bindAddress;
    private final L4Balance l4Balance;
    private final L4FrontListener l4FrontListener;
    private final Cluster cluster;
    private final CommonConfiguration commonConfiguration;

    private final ByteBufAllocator byteBufAllocator;
    private final EventLoopFactory eventLoopFactory;


    /**
     * @param bindAddress         {@link InetSocketAddress} on which {@link L4FrontListener} will bind and listen.
     * @param l4Balance           {@link L4Balance} for Load Balance
     * @param l4FrontListener     {@link L4FrontListener} for listening and handling traffic
     * @param cluster             {@link Cluster} to be Load Balanced
     * @param commonConfiguration {@link CommonConfiguration} to be applied
     * @throws NullPointerException If a required parameter if {@code null}
     */
    public L4LoadBalancer(InetSocketAddress bindAddress, L4Balance l4Balance, L4FrontListener l4FrontListener, Cluster cluster,
                          CommonConfiguration commonConfiguration) {
        this.bindAddress = Objects.requireNonNull(bindAddress, "bindAddress");
        this.l4Balance = Objects.requireNonNull(l4Balance, "L4Balance");
        this.l4FrontListener = Objects.requireNonNull(l4FrontListener, "L4FrontListener");
        this.cluster = Objects.requireNonNull(cluster, "Cluster");
        this.commonConfiguration = Objects.requireNonNull(commonConfiguration, "CommonConfiguration");
        this.byteBufAllocator = new PooledByteBufAllocator(commonConfiguration.getPooledByteBufAllocatorConfiguration()).getInstance();
        this.eventLoopFactory = new EventLoopFactory(commonConfiguration);
    }

    /**
     * Start L4 Load Balancer
     *
     * @return {@link List} containing {@link CompletableFuture} of {@link L4FrontListenerEvent}
     * which will notify when server is completely started.
     */
    public abstract List<CompletableFuture<L4FrontListenerEvent>> start();

    /**
     * Stop L4 Load Balancer
     *
     * @return {@link CompletableFuture} of {@link Boolean} which is set to {@code true} when server is successfully closed.
     */
    public abstract CompletableFuture<Boolean> stop();

    /**
     * Get {@link InetSocketAddress} on which {@link L4FrontListener} is bind.
     */
    public InetSocketAddress getBindAddress() {
        return bindAddress;
    }

    /**
     * Get {@link L4Balance} used to Load Balance
     */
    public L4Balance getL4Balance() {
        return l4Balance;
    }

    /**
     * Get {@link L4FrontListener} which is listening and handling traffic
     */
    public L4FrontListener getL4FrontListener() {
        return l4FrontListener;
    }

    /**
     * Get {@link Cluster} which is being Load Balanced
     */
    public Cluster getCluster() {
        return cluster;
    }

    /**
     * Get {@link CommonConfiguration} which is applied
     */
    public CommonConfiguration getCommonConfiguration() {
        return commonConfiguration;
    }

    /**
     * Get {@link ByteBufAllocator} created from {@link PooledByteBufAllocator}
     */
    public ByteBufAllocator getByteBufAllocator() {
        return byteBufAllocator;
    }

    /**
     * Get {@link EventLoopFactory} being used
     */
    public EventLoopFactory getEventLoopFactory() {
        return eventLoopFactory;
    }
}
