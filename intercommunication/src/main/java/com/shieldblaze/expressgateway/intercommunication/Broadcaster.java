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
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.util.SelfSignedCertificate;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public final class Broadcaster {

    static final Map<ByteBuf, InboundHandler> MEMBERS = new ConcurrentHashMap<>();

    static {
        System.setProperty("log4j.configurationFile", "log4j2.xml");
    }

    public static void main(String[] args) throws Exception {
        SelfSignedCertificate ssc = new SelfSignedCertificate("localhost", "EC", 256);
        SslContext sslContext = SslContextBuilder.forServer(ssc.key(), ssc.cert()).build();

        EventLoopGroup eventLoopGroup = new NioEventLoopGroup(2);

        ServerBootstrap bootstrap = new ServerBootstrap()
                .group(eventLoopGroup, eventLoopGroup)
                .channel(NioServerSocketChannel.class)
                .childHandler(new Initializer(sslContext));

        System.out.println(bootstrap.bind(9110).sync());
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
                    .addLast(new InboundHandler());
        }
    }
}
