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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.core.BootstrapFactory;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.ssl.SslHandler;

final class Bootstrapper {
    private final L4LoadBalancer l4LoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(L4LoadBalancer l4LoadBalancer, EventLoopGroup eventLoopGroup, ByteBufAllocator byteBufAllocator) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.eventLoopGroup = eventLoopGroup;
        this.byteBufAllocator = byteBufAllocator;
    }

    TCPConnection newInit(Node node, Channel channel) {
        int connectTimeout = l4LoadBalancer.coreConfiguration().transportConfiguration().backendConnectTimeout();
        TCPConnection tcpConnection = new TCPConnection(node, connectTimeout);

        Bootstrap bootstrap = BootstrapFactory.getTCP(l4LoadBalancer.coreConfiguration(), eventLoopGroup, byteBufAllocator)
                .handler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {

                        if (l4LoadBalancer.tlsForClient() != null) {
                            String hostname = node.socketAddress().getHostName();
                            int port = node.socketAddress().getPort();
                            SslHandler sslHandler = l4LoadBalancer.tlsForClient()
                                    .defaultMapping()
                                    .sslContext()
                                    .newHandler(ch.alloc(), hostname, port);

                            ch.pipeline().addFirst(sslHandler);
                        }

                        ch.pipeline().addLast(new DownstreamHandler(channel, node));
                    }
                });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        tcpConnection.init(channelFuture);

        channelFuture.addListener((ChannelFutureListener) future -> {
            if (!future.isSuccess()) {
                channel.close();
            }
        });

        return tcpConnection;
    }
}
