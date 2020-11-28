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
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.common.map.SelfExpiringMap;
import com.shieldblaze.expressgateway.common.utils.comparator.InetSocketAddressHashCodeComparator;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.socket.DatagramPacket;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.ConcurrentSkipListMap;

@ChannelHandler.Sharable
final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    private final Map<InetSocketAddress, UDPConnection> connectionMap;
    private final L4LoadBalancer l4LoadBalancer;
    private final Bootstrapper bootstrapper;

    UpstreamHandler(L4LoadBalancer l4LoadBalancer) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.bootstrapper = new Bootstrapper(l4LoadBalancer, l4LoadBalancer.eventLoopFactory().childGroup(), l4LoadBalancer.byteBufAllocator());
        connectionMap = new SelfExpiringMap<>(
                new ConcurrentSkipListMap<>(InetSocketAddressHashCodeComparator.INSTANCE),
                Duration.ofMillis(l4LoadBalancer.coreConfiguration().transportConfiguration().connectionIdleTimeout()),
                true
        );
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        l4LoadBalancer.eventLoopFactory().childGroup().next().execute(() -> {
            DatagramPacket datagramPacket = (DatagramPacket) msg;
            UDPConnection udpConnection = connectionMap.get(datagramPacket.sender());

            // If connection is null then we need to establish a new connection to the node.
            if (udpConnection == null) {
                Node node;
                try {
                    node = l4LoadBalancer.cluster().nextNode(new L4Request(datagramPacket.sender())).node();
                } catch (LoadBalanceException e) {
                    return;
                }

                udpConnection = bootstrapper.newInit(ctx.channel(), node, datagramPacket.sender());
                connectionMap.put(datagramPacket.sender(), udpConnection);
            }

            udpConnection.writeAndFlush(datagramPacket.content());
        });
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        logger.info("Closing All Upstream and Downstream Channels");
        connectionMap.forEach((socketAddress, udpConnection) -> udpConnection.close());
        connectionMap.clear();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
