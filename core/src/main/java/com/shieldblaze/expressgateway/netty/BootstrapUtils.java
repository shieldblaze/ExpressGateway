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
package com.shieldblaze.expressgateway.netty;

import com.shieldblaze.expressgateway.buffer.PooledByteBufAllocatorBuffer;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.ChannelOption;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollSocketChannel;
import io.netty.channel.socket.nio.NioSocketChannel;

public final class BootstrapUtils {

    private BootstrapUtils() {
        // Prevent outside initialization
    }

    public static Bootstrap tcp(EventLoopGroup eventLoopGroup) {
        return new Bootstrap()
                .group(eventLoopGroup)
                .option(ChannelOption.ALLOCATOR, PooledByteBufAllocatorBuffer.INSTANCE)
                .option(ChannelOption.SO_SNDBUF, 2147483647)
                .option(ChannelOption.TCP_NODELAY, true)
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
}
