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

import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerShutdownTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.core.factory.EventLoopFactory;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelOption;
import io.netty.channel.WriteBufferWaterMark;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.epoll.EpollServerSocketChannelConfig;
import io.netty.channel.group.ChannelGroup;
import io.netty.channel.group.DefaultChannelGroup;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.channel.unix.UnixChannelOption;
import io.netty.incubator.channel.uring.IOUringServerSocketChannel;
import io.netty.util.concurrent.GlobalEventExecutor;
import lombok.extern.log4j.Log4j2;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;

/**
 * TCP Listener for handling incoming TCP requests.
 */
@Log4j2
public class TCPListener extends L4FrontListener {

    /**
     * Default drain timeout in seconds. Active connections are given this
     * duration to complete before being forcefully closed on shutdown.
     */
    private static final int DEFAULT_DRAIN_TIMEOUT_SECONDS = 30;

    private final List<ChannelFuture> channelFutures = new CopyOnWriteArrayList<>();
    private final ChannelGroup activeConnections = new DefaultChannelGroup(GlobalEventExecutor.INSTANCE);
    private int drainTimeoutSeconds = DEFAULT_DRAIN_TIMEOUT_SECONDS;

    /**
     * HIGH-11: Set the drain timeout in seconds for graceful shutdown.
     * Active connections are given this duration to complete before being forcefully closed.
     *
     * @param seconds drain timeout in seconds, must be non-negative
     */
    public void setDrainTimeoutSeconds(int seconds) {
        this.drainTimeoutSeconds = Math.max(0, seconds);
    }

    /**
     * Get the active connections channel group for tracking.
     */
    ChannelGroup activeConnections() {
        return activeConnections;
    }

    @Override
    public L4FrontListenerStartupTask start() {
        L4FrontListenerStartupTask l4FrontListenerStartupEvent = new L4FrontListenerStartupTask();

        // If ChannelFutureList is not empty then this listener is already started and we won't start it again.
        if (!channelFutures.isEmpty()) {
            l4FrontListenerStartupEvent.markFailure(new IllegalArgumentException("Listener has already started and cannot be restarted."));
            return l4FrontListenerStartupEvent;
        }

        TransportConfiguration transportConfiguration = l4LoadBalancer().configurationContext().transportConfiguration();
        EventLoopFactory eventLoopFactory = l4LoadBalancer().eventLoopFactory();
        ByteBufAllocator byteBufAllocator = l4LoadBalancer().byteBufAllocator();

        ChannelHandler channelHandler;
        if (l4LoadBalancer().channelHandler() == null) {
            channelHandler = new ServerInitializer(l4LoadBalancer(), activeConnections);
        } else {
            channelHandler = l4LoadBalancer().channelHandler();
        }

        // MED-14: Normalize WriteBufferWaterMark across all transports
        WriteBufferWaterMark writeBufferWaterMark = new WriteBufferWaterMark(32 * 1024, 64 * 1024);

        ServerBootstrap serverBootstrap = new ServerBootstrap()
                .group(eventLoopFactory.parentGroup(), eventLoopFactory.childGroup())
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.recvByteBufAllocator())
                .option(ChannelOption.SO_RCVBUF, transportConfiguration.socketReceiveBufferSize())
                .option(ChannelOption.SO_SNDBUF, transportConfiguration.socketSendBufferSize())
                .option(ChannelOption.SO_BACKLOG, transportConfiguration.tcpConnectionBacklog())
                .option(ChannelOption.WRITE_BUFFER_WATER_MARK, writeBufferWaterMark)
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, true)
                .childOption(ChannelOption.SO_SNDBUF, transportConfiguration.socketSendBufferSize())
                .childOption(ChannelOption.SO_RCVBUF, transportConfiguration.socketReceiveBufferSize())
                .childOption(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.recvByteBufAllocator())
                .childOption(ChannelOption.TCP_NODELAY, true)
                .childOption(ChannelOption.SO_KEEPALIVE, true)  // MED-15: Detect dead clients via TCP keepalive
                .childOption(ChannelOption.ALLOW_HALF_CLOSURE, true) // RFC 9293 Sec 3.6: half-close support
                .childOption(ChannelOption.WRITE_BUFFER_WATER_MARK, writeBufferWaterMark) // MED-14: Consistent across transports
                ;

        // TCP_QUICKACK on accepted child channels — disable delayed ACK for lower latency.
        // Only available on native transports (Epoll, io_uring); NIO does not expose this option.
        if (transportConfiguration.transportType() == TransportType.EPOLL) {
            serverBootstrap.childOption(io.netty.channel.epoll.EpollChannelOption.TCP_QUICKACK, true);
        } else if (transportConfiguration.transportType() == TransportType.IO_URING) {
            serverBootstrap.childOption(io.netty.incubator.channel.uring.IOUringChannelOption.TCP_QUICKACK, true);
        }

        serverBootstrap.channelFactory(() -> {
                    if (transportConfiguration.transportType() == TransportType.IO_URING) {
                        IOUringServerSocketChannel serverSocketChannel = new IOUringServerSocketChannel();
                        serverSocketChannel.config().setOption(UnixChannelOption.SO_REUSEPORT, true);
                        serverSocketChannel.config().setTcpFastopen(transportConfiguration.tcpFastOpenMaximumPendingRequests());
                        return serverSocketChannel;
                    } else if (transportConfiguration.transportType() == TransportType.EPOLL) {
                        EpollServerSocketChannel serverSocketChannel = new EpollServerSocketChannel();
                        EpollServerSocketChannelConfig config = serverSocketChannel.config();
                        config.setOption(UnixChannelOption.SO_REUSEPORT, true);
                        config.setTcpFastopen(transportConfiguration.tcpFastOpenMaximumPendingRequests());
                        config.setEpollMode(EpollMode.EDGE_TRIGGERED);
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
    public L4FrontListenerStopTask stop() {
        L4FrontListenerStopTask l4FrontListenerStopEvent = new L4FrontListenerStopTask();

        // If ChannelFutureList is empty, then this listener is already stopped, and we won't stop it again.
        if (channelFutures.isEmpty()) {
            l4FrontListenerStopEvent.markFailure(new IllegalArgumentException("Listener has already stopped and cannot be stopped again."));
            return l4FrontListenerStopEvent;
        }

        // HIGH-11: Stop accepting new connections by closing server channels
        channelFutures.forEach(channelFuture -> channelFuture.channel().close());

        // Add a listener to last ChannelFuture to notify all listeners
        channelFutures.get(channelFutures.size() - 1).channel().closeFuture().addListener(future -> {
            if (future.isSuccess()) {
                int activeCount = activeConnections.size();
                if (activeCount > 0 && drainTimeoutSeconds > 0) {
                    // HIGH-11: Graceful drain -- wait for active connections to complete
                    log.info("Draining {} active connections (timeout: {}s)", activeCount, drainTimeoutSeconds);
                    GlobalEventExecutor.INSTANCE.schedule(() -> {
                        int remaining = activeConnections.size();
                        if (remaining > 0) {
                            log.info("Drain timeout reached, forcefully closing {} remaining connections", remaining);
                            activeConnections.close();
                        }
                        channelFutures.clear();
                        l4FrontListenerStopEvent.markSuccess(null);
                    }, drainTimeoutSeconds, TimeUnit.SECONDS);
                } else {
                    // No active connections or zero drain timeout -- close immediately
                    activeConnections.close().awaitUninterruptibly(5, TimeUnit.SECONDS);
                    channelFutures.clear();
                    l4FrontListenerStopEvent.markSuccess(null);
                }
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
    public L4FrontListenerShutdownTask shutdown() {
        L4FrontListenerStopTask event = stop();
        L4FrontListenerShutdownTask shutdownEvent = new L4FrontListenerShutdownTask();

        event.future().whenCompleteAsync((_void, throwable) -> {
            l4LoadBalancer().removeClusters();
            l4LoadBalancer().eventLoopFactory().parentGroup().shutdownGracefully();
            l4LoadBalancer().eventLoopFactory().childGroup().shutdownGracefully();
            shutdownEvent.markSuccess();
        }).thenRun(() -> l4LoadBalancer().eventStream().close());

        return shutdownEvent;
    }
}
