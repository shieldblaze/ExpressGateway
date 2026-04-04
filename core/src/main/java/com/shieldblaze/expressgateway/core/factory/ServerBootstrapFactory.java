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
package com.shieldblaze.expressgateway.core.factory;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelOption;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.WriteBufferWaterMark;
import io.netty.channel.epoll.EpollChannelOption;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.epoll.EpollServerSocketChannelConfig;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.channel.unix.UnixChannelOption;
import io.netty.incubator.channel.uring.IOUringChannelOption;
import io.netty.incubator.channel.uring.IOUringServerSocketChannel;

/**
 * Centralized factory for creating correctly-configured {@link ServerBootstrap} instances.
 *
 * <p>This eliminates duplicated ServerBootstrap construction across protocol modules
 * (TCP, HTTP) and ensures consistent socket tuning, backpressure configuration,
 * and transport-specific optimizations.</p>
 *
 * <h3>Configuration applied:</h3>
 * <ul>
 *   <li>Write buffer water marks (32KB low / 64KB high) for backpressure signaling</li>
 *   <li>TCP_NODELAY, SO_KEEPALIVE, ALLOW_HALF_CLOSURE on child channels</li>
 *   <li>TCP_QUICKACK on native transports (epoll, io_uring)</li>
 *   <li>TCP Fast Open on native server sockets</li>
 *   <li>SO_REUSEPORT on native transports for multi-listener binding</li>
 *   <li>Edge-triggered epoll mode for reduced syscall overhead</li>
 * </ul>
 */
public final class ServerBootstrapFactory {

    private static final WriteBufferWaterMark WRITE_BUFFER_WATER_MARK = new WriteBufferWaterMark(32 * 1024, 64 * 1024);

    @NonNull
    public static ServerBootstrap tcp(ConfigurationContext configurationContext,
                                      EventLoopGroup parentGroup,
                                      EventLoopGroup childGroup,
                                      ByteBufAllocator byteBufAllocator) {

        TransportConfiguration transportConfig = configurationContext.transportConfiguration();

        ServerBootstrap bootstrap = new ServerBootstrap()
                .group(parentGroup, childGroup)
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, transportConfig.recvByteBufAllocator())
                .option(ChannelOption.SO_RCVBUF, transportConfig.socketReceiveBufferSize())
                .option(ChannelOption.SO_SNDBUF, transportConfig.socketSendBufferSize())
                .option(ChannelOption.SO_BACKLOG, transportConfig.tcpConnectionBacklog())
                .option(ChannelOption.WRITE_BUFFER_WATER_MARK, WRITE_BUFFER_WATER_MARK)
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, true)
                .childOption(ChannelOption.ALLOCATOR, byteBufAllocator)
                .childOption(ChannelOption.SO_SNDBUF, transportConfig.socketSendBufferSize())
                .childOption(ChannelOption.SO_RCVBUF, transportConfig.socketReceiveBufferSize())
                .childOption(ChannelOption.RCVBUF_ALLOCATOR, transportConfig.recvByteBufAllocator())
                .childOption(ChannelOption.TCP_NODELAY, true)
                .childOption(ChannelOption.SO_KEEPALIVE, true)
                .childOption(ChannelOption.ALLOW_HALF_CLOSURE, true)
                .childOption(ChannelOption.WRITE_BUFFER_WATER_MARK, WRITE_BUFFER_WATER_MARK);

        // TCP_QUICKACK on child channels -- disable delayed ACK for lower latency.
        // Only available on native transports.
        if (transportConfig.transportType() == TransportType.EPOLL) {
            bootstrap.childOption(EpollChannelOption.TCP_QUICKACK, true);
        } else if (transportConfig.transportType() == TransportType.IO_URING) {
            bootstrap.childOption(IOUringChannelOption.TCP_QUICKACK, true);
        }

        bootstrap.channelFactory(() -> {
            if (transportConfig.transportType() == TransportType.IO_URING) {
                IOUringServerSocketChannel ch = new IOUringServerSocketChannel();
                ch.config().setOption(UnixChannelOption.SO_REUSEPORT, true);
                ch.config().setTcpFastopen(transportConfig.tcpFastOpenMaximumPendingRequests());
                return ch;
            } else if (transportConfig.transportType() == TransportType.EPOLL) {
                EpollServerSocketChannel ch = new EpollServerSocketChannel();
                EpollServerSocketChannelConfig config = ch.config();
                config.setOption(UnixChannelOption.SO_REUSEPORT, true);
                config.setTcpFastopen(transportConfig.tcpFastOpenMaximumPendingRequests());
                config.setEpollMode(EpollMode.EDGE_TRIGGERED);
                return ch;
            } else {
                return new NioServerSocketChannel();
            }
        });

        return bootstrap;
    }

    private ServerBootstrapFactory() {
        // Prevent outside initialization
    }
}
