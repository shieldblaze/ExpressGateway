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
package com.shieldblaze.expressgateway.server.udp;

import com.shieldblaze.expressgateway.netty.EventLoopUtils;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.socket.DatagramPacket;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArrayList;

final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private final List<Connection> connectionList = new CopyOnWriteArrayList<Connection>(){
        @Override
        public boolean add(Connection o) {
            super.add(o);
            Collections.sort(this);
            return true;
        }
    };

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        DatagramPacket datagramPacket = (DatagramPacket) msg;
        EventLoopUtils.CHILD.next().execute(() -> {
            int index = Collections.binarySearch(connectionList, datagramPacket.sender(), ConnectionSearchComparator.INSTANCE);

            if (index >= 0) {
                connectionList.get(index).writeDatagram(datagramPacket);
            } else {
                Connection connection = new Connection(datagramPacket.sender(), new InetSocketAddress("127.0.0.1", 9111), ctx.channel());
                connection.writeDatagram(datagramPacket);
                connectionList.add(connection);
            }
        });
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) throws Exception {
        cause.printStackTrace();
    }
}
