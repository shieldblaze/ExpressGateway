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
import com.shieldblaze.expressgateway.configuration.tls.TLSClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerConfiguration;
import com.shieldblaze.expressgateway.core.EventLoopFactory;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.PooledByteBufAllocator;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelHandler;

import java.net.InetSocketAddress;
import java.util.UUID;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * {@link L4LoadBalancer} holds base functions for a L4-Load Balancer.
 */
public abstract class L4LoadBalancer {

    public final String ID = UUID.randomUUID().toString();

    private final InetSocketAddress bindAddress;
    private final L4FrontListener l4FrontListener;
    private final Cluster cluster;
    private final CoreConfiguration coreConfiguration;
    private final TLSServerConfiguration tlsServerConfiguration;
    private final TLSClientConfiguration tlsClientConfiguration;
    private final ChannelHandler channelHandler;

    private final ByteBufAllocator byteBufAllocator;
    private final EventLoopFactory eventLoopFactory;

    private final AtomicBoolean running = new AtomicBoolean(false);

    /**
     * @param bindAddress            {@link InetSocketAddress} on which {@link L4FrontListener} will bind and listen.
     * @param l4FrontListener        {@link L4FrontListener} for listening traffic
     * @param cluster                {@link Cluster} to be Load Balanced
     * @param coreConfiguration      {@link CoreConfiguration} to be applied
     * @param tlsServerConfiguration {@link TLSServerConfiguration} for Server
     * @param tlsClientConfiguration {@link TLSClientConfiguration} for Client
     * @param channelHandler         {@link ChannelHandler} to use for handling traffic
     * @throws NullPointerException If a required parameter if {@code null}
     */
    public L4LoadBalancer(@NonNull InetSocketAddress bindAddress,
                          @NonNull L4FrontListener l4FrontListener,
                          @NonNull Cluster cluster,
                          @NonNull CoreConfiguration coreConfiguration,
                          TLSServerConfiguration tlsServerConfiguration,
                          TLSClientConfiguration tlsClientConfiguration,
                          ChannelHandler channelHandler) {
        this.bindAddress = bindAddress;
        this.l4FrontListener = l4FrontListener;
        this.cluster = cluster;
        this.coreConfiguration = coreConfiguration;
        this.tlsServerConfiguration = tlsServerConfiguration;
        this.tlsClientConfiguration = tlsClientConfiguration;
        this.channelHandler = channelHandler;

        this.byteBufAllocator = new PooledByteBufAllocator(coreConfiguration.bufferConfiguration()).Instance();
        this.eventLoopFactory = new EventLoopFactory(coreConfiguration);

        l4FrontListener.l4LoadBalancer(this);
    }

    /**
     * Start L4 Load Balancer
     */
    public L4FrontListenerStartupEvent start() {
        L4FrontListenerStartupEvent startupEvent = l4FrontListener.start();
        startupEvent.future().whenComplete((_void, throwable) -> {
            if (startupEvent.finished() && startupEvent.success()) {
                running.set(true);
            }
        });
        return startupEvent;
    }

    /**
     * Stop L4 Load Balancer
     */
    public L4FrontListenerStopEvent stop() {
        eventLoopFactory.parentGroup().shutdownGracefully();
        eventLoopFactory.childGroup().shutdownGracefully();

        L4FrontListenerStopEvent stopEvent = l4FrontListener.stop();
        stopEvent.future().whenComplete((unused, throwable) -> running.set(false));

        return stopEvent;
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
     * Get {@link TLSServerConfiguration} for Server
     */
    public TLSServerConfiguration tlsForServer() {
        return tlsServerConfiguration;
    }

    /**
     * Get {@link TLSClientConfiguration} for Client
     */
    public TLSClientConfiguration tlsForClient() {
        return tlsClientConfiguration;
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

    public AtomicBoolean running() {
        return running;
    }
}
