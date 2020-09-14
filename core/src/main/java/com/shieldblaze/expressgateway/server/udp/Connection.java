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

import com.google.common.primitives.SignedBytes;
import com.shieldblaze.expressgateway.netty.BootstrapUtils;
import com.shieldblaze.expressgateway.netty.EventLoopUtils;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.socket.DatagramPacket;

import java.net.InetSocketAddress;
import java.util.ArrayList;

final class Connection implements Comparable<Connection> {

    private final byte[] clientAddressAsBytes;
    private ArrayList<ByteBuf> datagramPacketBacklog = new ArrayList<>();
    private final Channel backendChannel;
    private boolean channelActive = false;

    Connection(InetSocketAddress clientAddress, InetSocketAddress destinationAddress, Channel clientChannel) {
        clientAddressAsBytes = AddressUtils.address(clientAddress);

        Bootstrap bootstrap = BootstrapUtils.udp(EventLoopUtils.CHILD);
        bootstrap.handler(new DownstreamHandler(clientChannel, clientAddress));
        ChannelFuture channelFuture = bootstrap.connect(destinationAddress);
        backendChannel = channelFuture.channel();
        channelFuture.addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                datagramPacketBacklog.forEach(byteBuf -> channelFuture.channel().writeAndFlush(new DatagramPacket(byteBuf, destinationAddress)));
                channelActive = true;
            } else {
                channelFuture.channel().close();
            }
            datagramPacketBacklog = null;
        });
    }

    @Override
    public int compareTo(Connection connection) {
        return SignedBytes.lexicographicalComparator().compare(clientAddressAsBytes, connection.clientAddressAsBytes);
    }

    void writeDatagram(ByteBuf byteBuf) {
        if (channelActive) {
            backendChannel.writeAndFlush(byteBuf);
        } else {
            datagramPacketBacklog.add(byteBuf);
        }
    }

    byte[] getClientAddressAsBytes() {
        return clientAddressAsBytes;
    }
}
