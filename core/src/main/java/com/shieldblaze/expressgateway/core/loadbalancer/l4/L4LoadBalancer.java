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

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Balance;
import com.shieldblaze.expressgateway.concurrent.eventstream.AsyncEventStream;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;
import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import com.shieldblaze.expressgateway.core.utils.EventLoopFactory;
import com.shieldblaze.expressgateway.core.utils.PooledByteBufAllocator;
import io.netty.buffer.ByteBufAllocator;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Executors;

/**
 * {@link L4LoadBalancer} holds base functions for a L4-Load Balancer.
 */
public abstract class L4LoadBalancer {

    private final AsyncEventStream eventStream = new AsyncEventStream(Executors.newFixedThreadPool(2));

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
    public L4LoadBalancer(InetSocketAddress bindAddress, L4Balance l4Balance, L4FrontListener l4FrontListener, Cluster cluster, CommonConfiguration commonConfiguration) {
        this.bindAddress = Objects.requireNonNull(bindAddress, "BindAddress");
        this.l4Balance = Objects.requireNonNull(l4Balance, "L4Balance");
        this.l4FrontListener = Objects.requireNonNull(l4FrontListener, "L4FrontListener");
        this.cluster = Objects.requireNonNull(cluster, "Cluster");
        this.commonConfiguration = Objects.requireNonNull(commonConfiguration, "CommonConfiguration");
        this.byteBufAllocator = new PooledByteBufAllocator(commonConfiguration.pooledByteBufAllocatorConfiguration()).instance();
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
    public InetSocketAddress bindAddress() {
        return bindAddress;
    }

    /**
     * Get {@link L4Balance} used to Load Balance
     */
    public L4Balance l4Balance() {
        return l4Balance;
    }

    /**
     * Get {@link L4FrontListener} which is listening and handling traffic
     */
    public L4FrontListener l4FrontListener() {
        return l4FrontListener;
    }

    /**
     * Get {@link Cluster} which is being Load Balanced
     */
    public Cluster cluster() {
        return cluster;
    }

    /**
     * Get {@link CommonConfiguration} which is applied
     */
    public CommonConfiguration commonConfiguration() {
        return commonConfiguration;
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

    /**
     * Subscribe to event stream
     *
     * @param eventListener {@link EventListener} implementation to receive events
     */
    public L4LoadBalancer subscribeToEvents(EventListener eventListener) {
        eventStream.subscribe(eventListener);
        return this;
    }

    /**
     * Unsubscribe from event stream
     *
     * @param eventListener {@link EventListener} implementation to stop which was subscribed
     */
    public L4LoadBalancer unsubscribeFromEvents(EventListener eventListener) {
        eventStream.unsubscribe(eventListener);
        return this;
    }
}
