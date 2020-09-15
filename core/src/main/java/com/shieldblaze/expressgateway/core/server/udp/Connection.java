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

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.core.netty.BootstrapFactory;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.DefaultAddressedEnvelope;
import io.netty.channel.socket.DatagramPacket;

import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentLinkedQueue;

final class Connection {

    private ConcurrentLinkedQueue<DatagramPacket> backlog = new ConcurrentLinkedQueue<>();

    private final Configuration configuration;
    private final Channel backendChannel;
    private final Backend backend;
    private boolean channelActive = false;

    Connection(InetSocketAddress clientAddress, Backend backend, Channel clientChannel, Configuration configuration,
               EventLoopFactory eventLoopFactory, ByteBufAllocator byteBufAllocator) {
        this.configuration = configuration;
        this.backend = backend;

        Bootstrap bootstrap = BootstrapFactory.getUDP(configuration, eventLoopFactory.getChildGroup(), byteBufAllocator);
        bootstrap.handler(new DownstreamHandler(clientChannel, clientAddress, backend));
        ChannelFuture channelFuture = bootstrap.connect(backend.getSocketAddress());
        backendChannel = channelFuture.channel();

        channelFuture.addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                eventLoopFactory.getChildGroup().next().execute(() -> {

                    backlog.forEach(datagramPacket -> {
                        backend.incBytesWritten(datagramPacket.content().readableBytes());
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
            backend.incBytesWritten(datagramPacket.content().readableBytes());
            backendChannel.writeAndFlush(datagramPacket.content()).addListener((ChannelFutureListener) cf -> {
                if (!cf.isSuccess()) {
                    datagramPacket.release();
                }
            });
            return;
        } else if (backlog != null) {
            if (backlog.size() < configuration.getTransportConfiguration().getDataBacklog()) {
                backlog.add(datagramPacket);
                return;
            }
        }
        datagramPacket.release();
    }
}
