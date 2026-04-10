/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.udp;

import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerShutdownTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.DatagramChannel;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * UDP Listener for handling incoming UDP requests.
 */
public class UDPListener extends L4FrontListener {

    private final List<ChannelFuture> channelFutures = new CopyOnWriteArrayList<>();

    @Override
    public L4FrontListenerStartupTask start() {
        L4FrontListenerStartupTask l4FrontListenerStartupEvent = new L4FrontListenerStartupTask();

        // If ChannelFutureList is not 0 then this listener is already started and we won't start it again.
        if (!channelFutures.isEmpty()) {
            l4FrontListenerStartupEvent.markFailure(new IllegalArgumentException("Listener has already started and cannot be restarted."));
            return l4FrontListenerStartupEvent;
        }

        ConfigurationContext configurationContext = l4LoadBalancer().configurationContext();
        EventLoopGroup eventLoopGroup = l4LoadBalancer().eventLoopFactory().parentGroup();

        ChannelHandler channelHandler;
        if (l4LoadBalancer().channelHandler() == null) {
            channelHandler = new UpstreamHandler(l4LoadBalancer());
        } else {
            channelHandler = l4LoadBalancer().channelHandler();
        }

        Bootstrap bootstrap = BootstrapFactory.udp(configurationContext, eventLoopGroup, l4LoadBalancer().byteBufAllocator())
                .handler(new ChannelInitializer<DatagramChannel>() {
                    @Override
                    protected void initChannel(DatagramChannel ch) {
                        ch.pipeline().addFirst(StandardEdgeNetworkMetricRecorder.INSTANCE);
                        ch.pipeline().addLast(channelHandler);
                    }
                });

        int bindRounds = 1;
        if (configurationContext.transportConfiguration().transportType().nativeTransport()) {
            bindRounds = configurationContext.eventLoopConfiguration().parentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            ChannelFuture channelFuture = bootstrap.bind(l4LoadBalancer().bindAddress());
            channelFutures.add(channelFuture);
        }

        // Add listener to last ChannelFuture to notify all listeners
        channelFutures.get(channelFutures.size() - 1).addListener(future -> {
            if (future.isSuccess()) {
                l4FrontListenerStartupEvent.markSuccess(null);
            } else {
                l4FrontListenerStartupEvent.markFailure(future.cause());
            }
        });

        l4LoadBalancer().eventStream().publish(l4FrontListenerStartupEvent);
        return l4FrontListenerStartupEvent;
    }

    @Override
    public L4FrontListenerStopTask stop() {
        L4FrontListenerStopTask l4FrontListenerStopEvent = new L4FrontListenerStopTask();

        if (channelFutures.isEmpty()) {
            l4FrontListenerStopEvent.markSuccess(null);
            return l4FrontListenerStopEvent;
        }

        // Close all ChannelFutures
        channelFutures.forEach(channelFuture -> channelFuture.channel().close());

        // Add a listener to last ChannelFuture to notify all listeners
        channelFutures.get(channelFutures.size() - 1).channel().closeFuture().addListener(future -> {
            if (future.isSuccess()) {
                channelFutures.clear();
                l4FrontListenerStopEvent.markSuccess(null);
            } else {
                l4FrontListenerStopEvent.markFailure(future.cause());
            }
        });

        // Shutdown Cluster
        l4LoadBalancer().clusters().forEach((_, cluster) -> cluster.close());
        l4LoadBalancer().eventStream().publish(l4FrontListenerStopEvent);
        return l4FrontListenerStopEvent;
    }

    @Override
    public L4FrontListenerShutdownTask shutdown() {
        L4FrontListenerStopTask event = stop();
        L4FrontListenerShutdownTask shutdownEvent = new L4FrontListenerShutdownTask();

        event.future().whenCompleteAsync((_, _) -> {
            l4LoadBalancer().removeClusters();
            l4LoadBalancer().eventLoopFactory().parentGroup().shutdownGracefully();
            l4LoadBalancer().eventLoopFactory().childGroup().shutdownGracefully();
            shutdownEvent.markSuccess(null);
        }, GlobalExecutors.executorService()).thenRun(() -> l4LoadBalancer().eventStream().close());

        return shutdownEvent;
    }
}
