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
package com.shieldblaze.expressgateway.protocol.udp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.EventLoopGroup;

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

    /**
     * MED-19: UDP source port preservation.
     * Ideally the backend channel would bind to the same source port as the client used
     * to reach the frontend, preserving the source port end-to-end (as AWS NLB and HAProxy do).
     * However, this is not possible when multiple clients use the same source port, or when
     * multiple backend connections are needed. The backend channel binds to an ephemeral port.
     * This is a known limitation documented in STANDARD_RELEASE.md.
     */
    UDPConnection newInit(Channel channel, Node node, InetSocketAddress socketAddress) {
        UDPConnection udpConnection = new UDPConnection(node);

        Bootstrap bootstrap = BootstrapFactory.udp(l4LoadBalancer.configurationContext(), eventLoopGroup, byteBufAllocator);
        bootstrap.handler(new ChannelInitializer<>() {
            @Override
            protected void initChannel(Channel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                // MED-17: Removed redundant ConnectionTimeoutHandler -- SelfExpiringMap
                // in UpstreamHandler already handles UDP session timeout via entry expiry.
                pipeline.addLast(new NodeBytesTracker(node));
                pipeline.addLast(new DownstreamHandler(channel, socketAddress, udpConnection));
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        udpConnection.init(channelFuture);
        return udpConnection;
    }
}
