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
package com.shieldblaze.expressgateway.protocol.udp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.core.BootstrapFactory;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.handler.logging.LogLevel;
import io.netty.handler.logging.LoggingHandler;
import io.netty.handler.timeout.IdleStateHandler;

import java.net.InetSocketAddress;

final class Bootstrapper {

    private final L4LoadBalancer l4LoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(L4LoadBalancer l4LoadBalancer, EventLoopGroup eventLoopGroup, ByteBufAllocator byteBufAllocator) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.eventLoopGroup = eventLoopGroup;
        this.byteBufAllocator = byteBufAllocator;
    }

    UDPConnection newInit(Channel channel, Node node, InetSocketAddress socketAddress) {
        int timeout = l4LoadBalancer.coreConfiguration().transportConfiguration().backendConnectTimeout();
        int connectionTimeout = l4LoadBalancer.coreConfiguration().transportConfiguration().connectionIdleTimeout();
        UDPConnection udpConnection = new UDPConnection(node, timeout);

        Bootstrap bootstrap = BootstrapFactory.getUDP(l4LoadBalancer.coreConfiguration(), eventLoopGroup, byteBufAllocator);
        bootstrap.handler(new ChannelInitializer<>() {
            @Override
            protected void initChannel(Channel ch) {
                ch.pipeline().addFirst(new LoggingHandler(LogLevel.DEBUG));
                ch.pipeline().addFirst(new IdleStateHandler(connectionTimeout, connectionTimeout, connectionTimeout));
                ch.pipeline().addLast(new DownstreamHandler(channel, node, socketAddress, udpConnection));
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        udpConnection.init(channelFuture);
        return udpConnection;
    }
}