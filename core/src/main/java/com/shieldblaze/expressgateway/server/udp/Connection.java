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
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.socket.DatagramPacket;
import io.netty.util.ReferenceCountUtil;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

final class Connection implements Comparable<Connection> {

    private List<DatagramPacket> datagramPacketBacklog = new ArrayList<>();
    private final byte[] clientAddressAsBytes;
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

                EventLoopUtils.CHILD.next().execute(() -> {
                    channelActive = true;

                    for (DatagramPacket datagramPacket : datagramPacketBacklog) {
                        if (datagramPacket == null) {
                            System.out.println("Packet got GCed");
                        } else {
                            backendChannel.writeAndFlush(datagramPacket.content());
                        }
                    }

                    datagramPacketBacklog.clear();
                });
            } else {
                channelFuture.channel().close();
                datagramPacketBacklog.clear();
            }
        });
    }

    @Override
    public int compareTo(Connection connection) {
        return SignedBytes.lexicographicalComparator().compare(clientAddressAsBytes, connection.clientAddressAsBytes);
    }

    void writeDatagram(DatagramPacket datagramPacket) {
        if (channelActive) {
            backendChannel.writeAndFlush(datagramPacket.content());
        } else {
            datagramPacketBacklog.add(datagramPacket);
        }
    }

    byte[] getClientAddressAsBytes() {
        return clientAddressAsBytes;
    }
}
