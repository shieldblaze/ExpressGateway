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
package com.shieldblaze.expressgateway.core.server.udp;

import com.shieldblaze.expressgateway.core.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.core.loadbalance.l4.L4Balance;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.socket.DatagramPacket;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private final Map<InetSocketAddress, Connection> connectionMap = new ConcurrentHashMap<>();
    private final L4Balance l4Balance;

    public UpstreamHandler(L4Balance l4Balance) {
        this.l4Balance = l4Balance;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        EventLoopFactory.CHILD.next().execute(() -> {
            DatagramPacket datagramPacket = (DatagramPacket) msg;
            Connection connection = connectionMap.get(datagramPacket.sender());

            if (connection == null) {
                Backend backend = l4Balance.getBackend(datagramPacket.sender());
                backend.incConnections();

                connection = new Connection(datagramPacket.sender(), backend, ctx.channel());
                connectionMap.put(datagramPacket.sender(), connection);
            }

            connection.writeDatagram(datagramPacket);
        });
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) throws Exception {
        cause.printStackTrace();
    }
}
