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

import com.shieldblaze.expressgateway.netty.BootstrapUtils;
import com.shieldblaze.expressgateway.netty.EventLoopFactory;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.DefaultAddressedEnvelope;
import io.netty.channel.socket.DatagramPacket;

import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentLinkedQueue;

final class Connection {

    private ConcurrentLinkedQueue<DatagramPacket> backlog = new ConcurrentLinkedQueue<>();
    final InetSocketAddress clientAddress;
    private final Channel backendChannel;
    private boolean channelActive = false;

    Connection(InetSocketAddress clientAddress, InetSocketAddress destinationAddress, Channel clientChannel) {
        this.clientAddress = clientAddress;

        Bootstrap bootstrap = BootstrapUtils.udp(EventLoopFactory.CHILD);
        bootstrap.handler(new DownstreamHandler(clientChannel, clientAddress));
        ChannelFuture channelFuture = bootstrap.connect(destinationAddress);
        backendChannel = channelFuture.channel();

        channelFuture.addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {

                EventLoopFactory.CHILD.next().execute(() -> {

                    backlog.forEach(datagramPacket -> {
                        backendChannel.writeAndFlush(datagramPacket.content()).addListener((ChannelFutureListener) cf -> {
                            if (!cf.isSuccess()) {
                                datagramPacket.release();
                            }
                        });
                    });

                    channelActive = true;
                    backlog = null;
                });
            } else {
                backlog.forEach(DefaultAddressedEnvelope::release);
                backlog = null;
                backendChannel.close();
            }
        });
    }

    void writeDatagram(DatagramPacket datagramPacket) {
        if (channelActive) {
            backendChannel.writeAndFlush(datagramPacket.content()).addListener((ChannelFutureListener) cf -> {
                if (!cf.isSuccess()) {
                    datagramPacket.release();
                }
            });
        } else if (backlog != null) {
            backlog.add(datagramPacket);
        }
    }
}
