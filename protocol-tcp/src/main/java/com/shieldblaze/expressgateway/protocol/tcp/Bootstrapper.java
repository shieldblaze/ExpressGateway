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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.ssl.SslHandler;

import java.time.Duration;

final class Bootstrapper {
    private final L4LoadBalancer l4LoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(L4LoadBalancer l4LoadBalancer) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.eventLoopGroup = l4LoadBalancer.eventLoopFactory().childGroup();
        this.byteBufAllocator = l4LoadBalancer.byteBufAllocator();
    }

    public TCPConnection newInit(Node node, Channel channel) {
        TCPConnection tcpConnection = new TCPConnection(node);

        Bootstrap bootstrap = BootstrapFactory.tcp(l4LoadBalancer.coreConfiguration(), eventLoopGroup, byteBufAllocator)
                .handler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        ChannelPipeline pipeline = ch.pipeline();

                        pipeline.addFirst(new NodeBytesTracker(node));

                        Duration timeout = Duration.ofMillis(l4LoadBalancer.coreConfiguration().transportConfiguration().connectionIdleTimeout());
                        pipeline.addLast(new ConnectionTimeoutHandler(timeout, false));

                        if (l4LoadBalancer.tlsForClient() != null) {
                            String hostname = node.socketAddress().getHostName();
                            int port = node.socketAddress().getPort();
                            SslHandler sslHandler = l4LoadBalancer.tlsForClient()
                                    .defaultMapping()
                                    .sslContext()
                                    .newHandler(ch.alloc(), hostname, port);

                            pipeline.addLast(sslHandler);
                        }

                        pipeline.addLast(new DownstreamHandler(channel, node));
                    }
                });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        tcpConnection.init(channelFuture);
        return tcpConnection;
    }
}
