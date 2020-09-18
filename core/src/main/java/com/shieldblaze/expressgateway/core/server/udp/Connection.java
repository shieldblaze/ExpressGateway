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

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.core.netty.BootstrapFactory;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.socket.DatagramPacket;
import io.netty.handler.timeout.IdleStateHandler;

import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicBoolean;

final class Connection {

    /**
     * Backlog of {@link DatagramPacket} pending to be written once connection with backend establishes
     */
    ConcurrentLinkedQueue<DatagramPacket> backlog = new ConcurrentLinkedQueue<>();

    private final CommonConfiguration commonConfiguration;
    private final Channel backendChannel;
    final InetSocketAddress clientAddress;
    final Backend backend;
    final AtomicBoolean connectionActive = new AtomicBoolean(true);
    private boolean channelActive = false;

    Connection(InetSocketAddress clientAddress, Backend backend, Channel clientChannel, CommonConfiguration commonConfiguration,
               EventLoopFactory eventLoopFactory, ByteBufAllocator byteBufAllocator) {
        this.commonConfiguration = commonConfiguration;
        this.clientAddress = clientAddress;
        this.backend = backend;

        Bootstrap bootstrap = BootstrapFactory.getUDP(commonConfiguration, eventLoopFactory.getChildGroup(), byteBufAllocator);
        bootstrap.handler(new DownstreamHandler(clientChannel, clientAddress, this));
        ChannelFuture channelFuture = bootstrap.connect(backend.getSocketAddress());
        backendChannel = channelFuture.channel();

        int timeout = commonConfiguration.getTransportConfiguration().getConnectionIdleTimeout();

        backendChannel.pipeline().addFirst(new IdleStateHandler(timeout, timeout, timeout));

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
                        backlog.remove(datagramPacket);
                    });

                    channelActive = true;
                    backlog = null;
                });
            } else {
                clearBacklog();
                backendChannel.close();
                connectionActive.set(false);
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
        } else if (backlog != null && backlog.size() < commonConfiguration.getTransportConfiguration().getDataBacklog()) {
            backlog.add(datagramPacket);
            return;
        }
        datagramPacket.release();
    }

    /**
     * Release all active {@link DatagramPacket} from {@link #backlog}
     */
    void clearBacklog() {
        if (backlog != null && backlog.size() > 0) {
            for (DatagramPacket datagramPacket : backlog) {
                if (datagramPacket.refCnt() > 0) {
                    datagramPacket.release();
                }
            }
        }
        backlog = null;
    }
}
