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
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelOption;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.EpollDatagramChannel;
import io.netty.channel.epoll.EpollDatagramChannelConfig;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollSocketChannel;
import io.netty.channel.socket.nio.NioDatagramChannel;
import io.netty.channel.socket.nio.NioSocketChannel;
import io.netty.channel.unix.UnixChannelOption;
import io.netty.incubator.channel.uring.IOUringChannelOption;
import io.netty.incubator.channel.uring.IOUringDatagramChannel;
import io.netty.incubator.channel.uring.IOUringSocketChannel;

/**
 * This class provides configured {@link Bootstrap} instances.
 */
public final class BootstrapFactory {

    @NonNull
    public static Bootstrap tcp(ConfigurationContext configurationContext, EventLoopGroup eventLoopGroup, ByteBufAllocator byteBufAllocator) {
        return new Bootstrap()
                .group(eventLoopGroup)
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, configurationContext.transportConfiguration().recvByteBufAllocator())
                .option(ChannelOption.SO_SNDBUF, configurationContext.transportConfiguration().socketSendBufferSize())
                .option(ChannelOption.SO_RCVBUF, configurationContext.transportConfiguration().socketReceiveBufferSize())
                .option(ChannelOption.TCP_NODELAY, true)
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.SO_KEEPALIVE, true)
                .option(ChannelOption.CONNECT_TIMEOUT_MILLIS, configurationContext.transportConfiguration().backendConnectTimeout())
                .channelFactory(() -> {
                    if (configurationContext.transportConfiguration().transportType() == TransportType.IO_URING) {
                        IOUringSocketChannel socketChannel = new IOUringSocketChannel();
                        socketChannel.config()
                                .setTcpFastOpenConnect(true)
                                .setTcpQuickAck(true);

                        return socketChannel;
                    } else if (configurationContext.transportConfiguration().transportType() == TransportType.EPOLL) {
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

    @NonNull
    public static Bootstrap udp(ConfigurationContext configurationContext, EventLoopGroup eventLoopGroup, ByteBufAllocator byteBufAllocator) {
        return new Bootstrap()
                .group(eventLoopGroup)
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, configurationContext.transportConfiguration().recvByteBufAllocator())
                .option(ChannelOption.SO_SNDBUF, configurationContext.transportConfiguration().socketSendBufferSize())
                .option(ChannelOption.SO_RCVBUF, configurationContext.transportConfiguration().socketReceiveBufferSize())
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, false)
                .channelFactory(() -> {
                    if (configurationContext.transportConfiguration().transportType() == TransportType.IO_URING) {
                        IOUringDatagramChannel datagramChannel = new IOUringDatagramChannel();
                        datagramChannel.config().setOption(IOUringChannelOption.SO_REUSEPORT, true);
                        return datagramChannel;
                    } else if (configurationContext.transportConfiguration().transportType() == TransportType.EPOLL) {
                        EpollDatagramChannel datagramChannel = new EpollDatagramChannel();

                        EpollDatagramChannelConfig config = datagramChannel.config();
                        config.setEpollMode(EpollMode.EDGE_TRIGGERED);
                        config.setOption(UnixChannelOption.SO_REUSEPORT, true);
                        config.setUdpGro(true);

                        return datagramChannel;
                    } else {
                        return new NioDatagramChannel();
                    }
                });
    }

    private BootstrapFactory() {
        // Prevent outside initialization
    }
}
