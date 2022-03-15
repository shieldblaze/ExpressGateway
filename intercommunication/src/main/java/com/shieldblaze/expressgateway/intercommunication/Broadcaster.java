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
package com.shieldblaze.expressgateway.intercommunication;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.PooledByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.ServerSocketChannel;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.handler.codec.DelimiterBasedFrameDecoder;
import io.netty.handler.ssl.*;
import io.netty.incubator.channel.uring.IOUring;
import io.netty.incubator.channel.uring.IOUringEventLoopGroup;
import io.netty.incubator.channel.uring.IOUringServerSocketChannel;

import javax.net.ssl.SSLException;
import java.net.InetAddress;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public final class Broadcaster {

    static final Map<ByteBuf, InboundHandler> MEMBERS = new ConcurrentHashMap<>();
    private static EventLoopGroup parentGroup;
    private static EventLoopGroup childGroup;
    private static ChannelFuture channelFuture;

    public static void start(int parentThreads, int childThreads, InetAddress address, int port, PrivateKey privateKey, X509Certificate... x509Certificates) throws SSLException {
        SslContext sslContext = SslContextBuilder.forServer(privateKey, x509Certificates)
                .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                .protocols("TLSv1.3")
                .ciphers(List.of("TLS_AES_256_GCM_SHA384"))
                .clientAuth(ClientAuth.NONE)
                .build();

        ServerSocketChannel serverSocketChannel;
        if (IOUring.isAvailable()) {
            parentGroup = new IOUringEventLoopGroup(parentThreads);
            childGroup = new IOUringEventLoopGroup(childThreads);
            serverSocketChannel = new IOUringServerSocketChannel();
        } else if (Epoll.isAvailable()) {
            parentGroup = new EpollEventLoopGroup(parentThreads);
            childGroup = new EpollEventLoopGroup(childThreads);
            serverSocketChannel = new EpollServerSocketChannel();
        } else {
            parentGroup = new NioEventLoopGroup(parentThreads);
            childGroup = new NioEventLoopGroup(childThreads);
            serverSocketChannel = new NioServerSocketChannel();
        }

        ServerBootstrap bootstrap = new ServerBootstrap()
                .group(parentGroup, childGroup)
                .channelFactory(() -> serverSocketChannel)
                .option(ChannelOption.ALLOCATOR, PooledByteBufAllocator.DEFAULT)
                .option(ChannelOption.SO_KEEPALIVE, true)
                .option(ChannelOption.TCP_NODELAY, true)
                .childHandler(new Initializer(sslContext));

        channelFuture = bootstrap.bind(address, port);
    }

    public static void stop() {
        channelFuture.channel().close();
        parentGroup.shutdownGracefully();
        childGroup.shutdownGracefully();
    }

    /**
     * Initializer for initializing pipeline
     */
    private static final class Initializer extends ChannelInitializer<SocketChannel> {

        private final SslContext sslContext;

        @NonNull
        private Initializer(SslContext sslContext) {
            this.sslContext = sslContext;
        }

        @Override
        protected void initChannel(SocketChannel ch) {
            ch.pipeline()
                    .addFirst(sslContext.newHandler(ch.alloc()))
                    .addLast(new DelimiterBasedFrameDecoder(10_000_000, Messages.DELIMITER()))
                    .addLast(new Encoder())
                    .addLast(new Decoder())
                    .addLast(new InboundHandler());
        }
    }
}
