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
package com.shieldblaze.expressgateway.server.tcp;

import com.shieldblaze.expressgateway.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.netty.PooledByteBufAllocatorBuffer;
import com.shieldblaze.expressgateway.server.FrontListener;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.channel.AdaptiveRecvByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

public final class Listener extends FrontListener {

    private static final Logger logger = LogManager.getLogger(Listener.class);

    public Listener(InetSocketAddress bindAddress) {
        super(bindAddress);
    }

    @Override
    public void start() {

        ServerBootstrap serverBootstrap = new ServerBootstrap()
                .group(EventLoopFactory.PARENT, EventLoopFactory.CHILD)
                .option(ChannelOption.ALLOCATOR, PooledByteBufAllocatorBuffer.INSTANCE)
                .option(ChannelOption.RCVBUF_ALLOCATOR, new AdaptiveRecvByteBufAllocator(1500, 9001, 65536))
                .option(ChannelOption.SO_RCVBUF, 2147483647)
                .option(ChannelOption.SO_BACKLOG, 2147483647)
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, false)
                .childOption(ChannelOption.SO_SNDBUF, 2147483647)
                .childOption(ChannelOption.SO_RCVBUF, 2147483647)
                .childOption(ChannelOption.RCVBUF_ALLOCATOR, new AdaptiveRecvByteBufAllocator(1500, 9001, 65536))
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

        ChannelFuture channelFuture = serverBootstrap.bind(bindAddress).addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                logger.atInfo().log("Server Successfully Started at: {}", future.channel().localAddress());
            }
        });

        channelFutureList.add(channelFuture);
    }

    private static final class ServerInitializer extends ChannelInitializer<SocketChannel> {

        @Override
        protected void initChannel(SocketChannel socketChannel) {
            socketChannel.pipeline().addFirst(new UpstreamHandler());
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) throws Exception {
            System.out.println(cause.getMessage());
        }
    }
}
