/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerShutdownEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.metrics.EdgeNetworkMetricRecorder;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.DatagramChannel;

import java.util.ArrayList;
import java.util.List;

/**
 * UDP Listener for handling incoming UDP requests.
 */
public class UDPListener extends L4FrontListener {

    private final List<ChannelFuture> channelFutures = new ArrayList<>();

    @Override
    public L4FrontListenerStartupEvent start() {
        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = new L4FrontListenerStartupEvent();

        // If ChannelFutureList is not 0 then this listener is already started and we won't start it again.
        if (channelFutures.size() != 0) {
            l4FrontListenerStartupEvent.tryFailure(new IllegalArgumentException("Listener has already started and cannot be restarted."));
            return l4FrontListenerStartupEvent;
        }

        CoreConfiguration coreConfiguration = l4LoadBalancer().coreConfiguration();
        EventLoopGroup eventLoopGroup = l4LoadBalancer().eventLoopFactory().parentGroup();

        ChannelHandler channelHandler;
        if (l4LoadBalancer().channelHandler() == null) {
            channelHandler = new UpstreamHandler(l4LoadBalancer());
        } else {
            channelHandler = l4LoadBalancer().channelHandler();
        }

        Bootstrap bootstrap = BootstrapFactory.udp(coreConfiguration, eventLoopGroup, l4LoadBalancer().byteBufAllocator())
                .handler(new ChannelInitializer<DatagramChannel>() {
                    @Override
                    protected void initChannel(DatagramChannel ch) {
                        ch.pipeline().addFirst(EdgeNetworkMetricRecorder.INSTANCE);
                        ch.pipeline().addLast(channelHandler);
                    }
                });

        int bindRounds = 1;
        if (coreConfiguration.transportConfiguration().transportType().nativeTransport()) {
            bindRounds = coreConfiguration.eventLoopConfiguration().parentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            ChannelFuture channelFuture = bootstrap.bind(l4LoadBalancer().bindAddress());
            channelFutures.add(channelFuture);
        }

        // Add listener to last ChannelFuture to notify all listeners
        channelFutures.get(channelFutures.size() - 1).addListener(future -> {
            if (future.isSuccess()) {
                l4FrontListenerStartupEvent.trySuccess(null);
            } else {
                l4FrontListenerStartupEvent.tryFailure(future.cause());
            }
        });

        l4LoadBalancer().eventStream().publish(l4FrontListenerStartupEvent);
        return l4FrontListenerStartupEvent;
    }

    @Override
    public L4FrontListenerStopEvent stop() {
        L4FrontListenerStopEvent l4FrontListenerStopEvent = new L4FrontListenerStopEvent();

        channelFutures.forEach(channelFuture -> channelFuture.channel().close());
        channelFutures.get(channelFutures.size() - 1).channel().closeFuture().addListener(future -> {
            if (future.isSuccess()) {
                l4FrontListenerStopEvent.trySuccess(null);
            } else {
                l4FrontListenerStopEvent.tryFailure(future.cause());
            }
        });

        // Shutdown Cluster
        l4LoadBalancer().clusters().forEach((str, cluster) -> cluster.close());
        l4LoadBalancer().eventStream().publish(l4FrontListenerStopEvent);
        return l4FrontListenerStopEvent;
    }

    @Override
    public L4FrontListenerShutdownEvent shutdown() {
        L4FrontListenerStopEvent event = stop();
        L4FrontListenerShutdownEvent shutdownEvent = new L4FrontListenerShutdownEvent();

        event.future().whenCompleteAsync((_void, throwable) -> {
            l4LoadBalancer().clusters().clear();
            l4LoadBalancer().eventLoopFactory().parentGroup().shutdownGracefully();
            l4LoadBalancer().eventLoopFactory().childGroup().shutdownGracefully();
            shutdownEvent.trySuccess(null);
        }, GlobalExecutors.INSTANCE.executorService());

        return shutdownEvent;
    }
}
