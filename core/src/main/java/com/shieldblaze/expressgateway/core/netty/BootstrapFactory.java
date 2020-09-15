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
package com.shieldblaze.expressgateway.core.netty;

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.buffer.PooledByteBufAllocator;
import io.netty.channel.AdaptiveRecvByteBufAllocator;
import io.netty.channel.ChannelOption;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollDatagramChannel;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollSocketChannel;
import io.netty.channel.socket.nio.NioDatagramChannel;
import io.netty.channel.socket.nio.NioSocketChannel;
import io.netty.channel.unix.UnixChannelOption;

public final class BootstrapFactory {

    private BootstrapFactory() {
        // Prevent outside initialization
    }

    public static Bootstrap getTCP(Configuration configuration, EventLoopGroup eventLoopGroup, ByteBufAllocator byteBufAllocator) {
        return new Bootstrap()
                .group(eventLoopGroup)
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, configuration.getTransportConfiguration().getRecvByteBufAllocator())
                .option(ChannelOption.SO_SNDBUF, configuration.getTransportConfiguration().getSocketSendBufferSize())
                .option(ChannelOption.SO_RCVBUF, configuration.getTransportConfiguration().getSocketReceiveBufferSize())
                .option(ChannelOption.TCP_NODELAY, true)
                .option(ChannelOption.CONNECT_TIMEOUT_MILLIS, configuration.getTransportConfiguration().getBackendConnectTimeout())
                .channelFactory(() -> {
                    if (Epoll.isAvailable()) {
                        EpollSocketChannel socketChannel = new EpollSocketChannel();
                        socketChannel.config()
                                .setEpollMode(EpollMode.EDGE_TRIGGERED)
                                .setTcpFastOpenConnect(true)
                                .setTcpQuickAck(true);

                        return socketChannel;
                    } else {
                        return new NioSocketChannel();
                    }
                });
    }

    public static Bootstrap getUDP(Configuration configuration, EventLoopGroup eventLoopGroup, ByteBufAllocator byteBufAllocator) {
        return new Bootstrap()
                .group(eventLoopGroup)
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, configuration.getTransportConfiguration().getRecvByteBufAllocator())
                .option(ChannelOption.SO_SNDBUF, configuration.getTransportConfiguration().getSocketSendBufferSize())
                .option(ChannelOption.SO_RCVBUF, configuration.getTransportConfiguration().getSocketReceiveBufferSize())
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, false)
                .channelFactory(() -> {
                    if (Epoll.isAvailable()) {
                        EpollDatagramChannel epollDatagramChannel = new EpollDatagramChannel();
                        epollDatagramChannel.config()
                                .setEpollMode(EpollMode.EDGE_TRIGGERED)
                                .setOption(UnixChannelOption.SO_REUSEPORT, true);
                        return epollDatagramChannel;
                    } else {
                        return new NioDatagramChannel();
                    }
                });
    }
}
