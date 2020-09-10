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
package com.shieldblaze.expressgateway.listener.server;

import com.shieldblaze.expressgateway.buffer.PooledByteBufAllocatorBuffer;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.FixedRecvByteBufAllocator;
import io.netty.channel.ServerChannel;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

public final class TCP extends Thread {

    private static final Logger logger = LogManager.getLogger(TCP.class);

    public TCP() {
        super("TCP-Server-Listener");
    }

    @Override
    public void run() {
        EventLoopGroup parentGroup;
        EventLoopGroup childGroup;

        if (Epoll.isAvailable()) {
            parentGroup = new EpollEventLoopGroup(2);
            childGroup = new EpollEventLoopGroup(4);
        } else {
            parentGroup = new NioEventLoopGroup(2);
            childGroup = new NioEventLoopGroup(4);
        }

        ServerBootstrap serverBootstrap = new ServerBootstrap()
                .group(parentGroup, childGroup)
                .option(ChannelOption.ALLOCATOR, PooledByteBufAllocatorBuffer.INSTANCE)
                .option(ChannelOption.RCVBUF_ALLOCATOR, new FixedRecvByteBufAllocator(65535))
                .option(ChannelOption.SO_TIMEOUT, 2500)
                .option(ChannelOption.SO_RCVBUF, 2147483647)
                .option(ChannelOption.SO_BACKLOG, 2147483647)
                .option(ChannelOption.SO_KEEPALIVE, true)
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, false)
                .channelFactory(() -> {
                    if (Epoll.isAvailable()) {
                        EpollServerSocketChannel serverSocketChannel = new EpollServerSocketChannel();
                        serverSocketChannel.config()
                                .setEpollMode(EpollMode.EDGE_TRIGGERED)
                                .setTcpFastopen(2147483647);

                        return serverSocketChannel;
                    } else {
                        return new NioServerSocketChannel();
                    }
                })
                .childHandler(new ServerInitializer());

        serverBootstrap.bind(new InetSocketAddress("0.0.0.0", 9110))
                .addListener((ChannelFutureListener) future -> {
                    if (future.isSuccess()) {
                        logger.atInfo().log("Server Successfully Started at: {}", future.channel().localAddress());
                    }
                });
    }

    private static final class ServerInitializer extends ChannelInitializer<ServerChannel> {

        @Override
        protected void initChannel(ServerChannel serverChannel) {
            serverChannel.pipeline().addFirst(new UpstreamHandler());
        }
    }
}
