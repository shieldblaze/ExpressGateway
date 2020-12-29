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
package com.shieldblaze.expressgateway.core.loadbalancer;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventPublisher;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventSubscriber;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.EventLoopFactory;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.PooledByteBufAllocator;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelHandler;

import java.net.InetSocketAddress;
import java.util.UUID;

/**
 * {@link L4LoadBalancer} holds base functions for a L4-Load Balancer.
 */
public abstract class L4LoadBalancer {

    public final String ID = UUID.randomUUID().toString();

    private final InetSocketAddress bindAddress;
    private final L4FrontListener l4FrontListener;
    private final Cluster cluster;
    private final CoreConfiguration coreConfiguration;
    private final TLSConfiguration tlsForServer;
    private final TLSConfiguration tlsForClient;
    private final ChannelHandler channelHandler;

    private final ByteBufAllocator byteBufAllocator;
    private final EventLoopFactory eventLoopFactory;

    /**
     * @param bindAddress       {@link InetSocketAddress} on which {@link L4FrontListener} will bind and listen.
     * @param l4FrontListener   {@link L4FrontListener} for listening traffic
     * @param cluster           {@link Cluster} to be Load Balanced
     * @param coreConfiguration {@link CoreConfiguration} to be applied
     * @param tlsForServer      {@link TLSConfiguration} for Server
     * @param tlsForClient      {@link TLSConfiguration} for Client
     * @param channelHandler    {@link ChannelHandler} to use for handling traffic
     * @throws NullPointerException If a required parameter if {@code null}
     */
    public L4LoadBalancer(@NonNull InetSocketAddress bindAddress,
                          @NonNull L4FrontListener l4FrontListener,
                          @NonNull Cluster cluster,
                          @NonNull CoreConfiguration coreConfiguration,
                          TLSConfiguration tlsForServer,
                          TLSConfiguration tlsForClient,
                          ChannelHandler channelHandler) {
        this.bindAddress = bindAddress;
        this.l4FrontListener = l4FrontListener;
        this.cluster = cluster;
        this.coreConfiguration = coreConfiguration;
        this.tlsForServer = tlsForServer;
        this.tlsForClient = tlsForClient;
        this.channelHandler = channelHandler;

        this.byteBufAllocator = new PooledByteBufAllocator(coreConfiguration.pooledByteBufAllocatorConfiguration()).Instance();
        this.eventLoopFactory = new EventLoopFactory(coreConfiguration);

        l4FrontListener.l4LoadBalancer(this);
    }

    /**
     * Start L4 Load Balancer
     */
    public L4FrontListenerStartupEvent start() {
        return l4FrontListener.start();
    }

    /**
     * Stop L4 Load Balancer
     */
    public L4FrontListenerStopEvent stop() {
        eventLoopFactory.parentGroup().shutdownGracefully();
        eventLoopFactory.childGroup().shutdownGracefully();
        return l4FrontListener.stop();
    }

    /**
     * Get {@link InetSocketAddress} on which {@link L4FrontListener} is bind.
     */
    public InetSocketAddress bindAddress() {
        return bindAddress;
    }

    /**
     * Get {@link Cluster} which is being Load Balanced
     */
    public Cluster cluster() {
        return cluster;
    }

    /**
     * Get {@link CoreConfiguration} which is applied
     */
    public CoreConfiguration coreConfiguration() {
        return coreConfiguration;
    }

    /**
     * Get {@link TLSConfiguration} for Server
     */
    public TLSConfiguration tlsForServer() {
        return tlsForServer;
    }

    /**
     * Get {@link TLSConfiguration} for Client
     */
    public TLSConfiguration tlsForClient() {
        return tlsForClient;
    }

    /**
     * Get {@link ChannelHandler} used for handling traffic
     */
    public ChannelHandler channelHandler() {
        return channelHandler;
    }

    /**
     * Get {@link ByteBufAllocator} created from {@link PooledByteBufAllocator}
     */
    public ByteBufAllocator byteBufAllocator() {
        return byteBufAllocator;
    }

    /**
     * Get {@link EventLoopFactory} being used
     */
    public EventLoopFactory eventLoopFactory() {
        return eventLoopFactory;
    }

    public EventPublisher eventPublisher() {
        return cluster.eventPublisher();
    }

    public EventSubscriber eventSubscriber() {
        return cluster.eventSubscriber();
    }
}
