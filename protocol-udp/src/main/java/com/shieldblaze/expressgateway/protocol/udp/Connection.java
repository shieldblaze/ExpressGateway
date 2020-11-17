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
import com.shieldblaze.expressgateway.common.utils.ReferenceCounted;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.core.BootstrapFactory;
import com.shieldblaze.expressgateway.core.EventLoopFactory;
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

    private final CoreConfiguration coreConfiguration;
    private final Channel backendChannel;
    final InetSocketAddress clientAddress;
    final Node node;
    final AtomicBoolean connectionActive = new AtomicBoolean(true);
    private boolean channelActive = false;

    Connection(InetSocketAddress clientAddress, Node node, Channel clientChannel, CoreConfiguration coreConfiguration,
               EventLoopFactory eventLoopFactory, ByteBufAllocator byteBufAllocator) {
        this.coreConfiguration = coreConfiguration;
        this.clientAddress = clientAddress;
        this.node = node;

        Bootstrap bootstrap = BootstrapFactory.getUDP(coreConfiguration, eventLoopFactory.childGroup(), byteBufAllocator);
        bootstrap.handler(new DownstreamHandler(clientChannel, clientAddress, this));
        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        backendChannel = channelFuture.channel();

        int timeout = coreConfiguration.transportConfiguration().connectionIdleTimeout();

        backendChannel.pipeline().addFirst(new IdleStateHandler(timeout, timeout, timeout));

        channelFuture.addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                eventLoopFactory.childGroup().next().execute(() -> {

                    backlog.forEach(datagramPacket -> {
                        node.incBytesWritten(datagramPacket.content().readableBytes());
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
            node.incBytesWritten(datagramPacket.content().readableBytes());
            backendChannel.writeAndFlush(datagramPacket.content()).addListener((ChannelFutureListener) cf -> {
                if (!cf.isSuccess()) {
                    ReferenceCounted.silentFullRelease(datagramPacket);
                }
            });
            return;
        } else if (backlog != null && backlog.size() < coreConfiguration.transportConfiguration().dataBacklog()) {
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
                ReferenceCounted.silentFullRelease(datagramPacket);
            }
        }
        backlog = null;
    }
}
