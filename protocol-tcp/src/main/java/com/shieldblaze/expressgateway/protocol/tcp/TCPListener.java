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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerShutdownEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.core.factory.EventLoopFactory;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelOption;
import io.netty.channel.WriteBufferWaterMark;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.epoll.EpollServerSocketChannelConfig;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.channel.unix.UnixChannelOption;
import io.netty.incubator.channel.uring.IOUringChannelOption;
import io.netty.incubator.channel.uring.IOUringServerSocketChannel;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * TCP Listener for handling incoming TCP requests.
 */
public class TCPListener extends L4FrontListener {

    private final List<ChannelFuture> channelFutures = new CopyOnWriteArrayList<>();

    @Override
    public L4FrontListenerStartupEvent start() {
        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = new L4FrontListenerStartupEvent();

        // If ChannelFutureList is not 0 then this listener is already started and we won't start it again.
        if (channelFutures.size() != 0) {
            l4FrontListenerStartupEvent.markFailure(new IllegalArgumentException("Listener has already started and cannot be restarted."));
            return l4FrontListenerStartupEvent;
        }

        TransportConfiguration transportConfiguration = l4LoadBalancer().configurationContext().transportConfiguration();
        EventLoopFactory eventLoopFactory = l4LoadBalancer().eventLoopFactory();
        ByteBufAllocator byteBufAllocator = l4LoadBalancer().byteBufAllocator();

        ChannelHandler channelHandler;
        if (l4LoadBalancer().channelHandler() == null) {
            channelHandler = new ServerInitializer(l4LoadBalancer());
        } else {
            channelHandler = l4LoadBalancer().channelHandler();
        }

        ServerBootstrap serverBootstrap = new ServerBootstrap()
                .group(eventLoopFactory.parentGroup(), eventLoopFactory.childGroup())
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.recvByteBufAllocator())
                .option(ChannelOption.SO_RCVBUF, transportConfiguration.socketReceiveBufferSize())
                .option(ChannelOption.SO_BACKLOG, transportConfiguration.tcpConnectionBacklog())
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, true)
                .childOption(ChannelOption.SO_SNDBUF, transportConfiguration.socketSendBufferSize())
                .childOption(ChannelOption.SO_RCVBUF, transportConfiguration.socketReceiveBufferSize())
                .childOption(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.recvByteBufAllocator())
                .channelFactory(() -> {
                    if (transportConfiguration.transportType() == TransportType.IO_URING) {
                        IOUringServerSocketChannel serverSocketChannel = new IOUringServerSocketChannel();
                        serverSocketChannel.config().setOption(IOUringChannelOption.SO_REUSEPORT, true);
                        return serverSocketChannel;
                    } else if (transportConfiguration.transportType() == TransportType.EPOLL) {
                        EpollServerSocketChannel serverSocketChannel = new EpollServerSocketChannel();
                        EpollServerSocketChannelConfig config = serverSocketChannel.config();
                        config.setOption(UnixChannelOption.SO_REUSEPORT, true);
                        config.setTcpFastopen(transportConfiguration.tcpFastOpenMaximumPendingRequests());
                        config.setEpollMode(EpollMode.EDGE_TRIGGERED);
                        config.setWriteBufferWaterMark(new WriteBufferWaterMark(0, Integer.MAX_VALUE));
                        config.setPerformancePreferences(100, 100, 100);

                        return serverSocketChannel;
                    } else {
                        return new NioServerSocketChannel();
                    }
                })
                .childHandler(channelHandler);

        int bindRounds = 1;
        if (transportConfiguration.transportType().nativeTransport()) {
            bindRounds = l4LoadBalancer().configurationContext().eventLoopConfiguration().parentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            ChannelFuture channelFuture = serverBootstrap.bind(l4LoadBalancer().bindAddress());
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
    public L4FrontListenerStopEvent stop() {
        L4FrontListenerStopEvent l4FrontListenerStopEvent = new L4FrontListenerStopEvent();

        channelFutures.forEach(channelFuture -> channelFuture.channel().close());
        channelFutures.get(channelFutures.size() - 1).channel().closeFuture().addListener(future -> {
            if (future.isSuccess()) {
                channelFutures.clear();
                l4FrontListenerStopEvent.markSuccess(null);
            } else {
                l4FrontListenerStopEvent.markFailure(future.cause());
            }
        });

        // Shutdown Cluster
        l4LoadBalancer().clusters().forEach((hostname, cluster) -> cluster.close());
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
            shutdownEvent.markSuccess();
        }, GlobalExecutors.executorService());

        return shutdownEvent;
    }
}
