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
package com.shieldblaze.expressgateway.core.l4;

import com.shieldblaze.expressgateway.core.concurrent.async.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.core.netty.PooledByteBufAllocator;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Balance;
import io.netty.buffer.ByteBufAllocator;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;

public abstract class AbstractL4LoadBalancer {

    private final InetSocketAddress bindAddress;
    private final L4Balance l4Balance;
    private final L4FrontListener l4FrontListener;
    private final Cluster cluster;
    private final CommonConfiguration commonConfiguration;

    private final ByteBufAllocator byteBufAllocator;
    private final EventLoopFactory eventLoopFactory;


    public AbstractL4LoadBalancer(InetSocketAddress bindAddress, L4Balance l4Balance, L4FrontListener l4FrontListener, Cluster cluster,
                                  CommonConfiguration commonConfiguration) {
        this.bindAddress = Objects.requireNonNull(bindAddress, "bindAddress");
        this.l4Balance = Objects.requireNonNull(l4Balance, "L4Balance");
        this.l4FrontListener = Objects.requireNonNull(l4FrontListener, "L4FrontListener");
        this.cluster = Objects.requireNonNull(cluster, "Cluster");
        this.commonConfiguration = Objects.requireNonNull(commonConfiguration, "CommonConfiguration");
        this.byteBufAllocator = new PooledByteBufAllocator(commonConfiguration.getPooledByteBufAllocatorConfiguration()).getInstance();
        this.eventLoopFactory = new EventLoopFactory(commonConfiguration);
    }

    public abstract List<CompletableFuture<L4FrontListenerEvent>> start();

    public abstract CompletableFuture<Boolean> stop();

    public InetSocketAddress getBindAddress() {
        return bindAddress;
    }

    public L4Balance getL4Balance() {
        return l4Balance;
    }

    public L4FrontListener getL4FrontListener() {
        return l4FrontListener;
    }

    public Cluster getCluster() {
        return cluster;
    }

    public CommonConfiguration getCommonConfiguration() {
        return commonConfiguration;
    }

    public ByteBufAllocator getByteBufAllocator() {
        return byteBufAllocator;
    }

    public EventLoopFactory getEventLoopFactory() {
        return eventLoopFactory;
    }
}
