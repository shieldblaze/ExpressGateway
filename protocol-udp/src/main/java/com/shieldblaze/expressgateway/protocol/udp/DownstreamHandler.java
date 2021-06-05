/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.socket.DatagramPacket;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final Channel channel;
    private final UDPConnection udpConnection;
    private final InetSocketAddress socketAddress;

    DownstreamHandler(Channel channel, InetSocketAddress socketAddress, UDPConnection udpConnection) {
        this.channel = channel;
        this.udpConnection = udpConnection;
        this.socketAddress = socketAddress;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        DatagramPacket packet = (DatagramPacket) msg;
        channel.writeAndFlush(new DatagramPacket(packet.content(), socketAddress), channel.voidPromise()); // Write Data back to Client
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (logger.isDebugEnabled()) {
            logger.debug("Closing Upstream {} and Downstream {} Channel",
                    socketAddress.getAddress().getHostAddress() + ":" + socketAddress.getPort(),
                    udpConnection.socketAddress().getAddress().getHostAddress() + ":" + udpConnection.socketAddress().getPort());
        }

        udpConnection.close();
        ctx.channel().close(); // Close Downstream Channel
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
