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

import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.socket.DatagramPacket;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

/**
 * <p> Downstream Handler receives Data from Backend.
 *
 * <p> Flow: </p>
 * <p> &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 * &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 * &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 * &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 * &nbsp; &nbsp;(Data) </p>
 * (INTERNET) --<--<--<--< (EXPRESSGATEWAY) --<--<--<--< (BACKEND)
 */
final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final Channel clientChannel;
    private final InetSocketAddress clientAddress;
    private final Connection connection;

    DownstreamHandler(Channel clientChannel, InetSocketAddress clientAddress, Connection connection) {
        this.clientChannel = clientChannel;
        this.clientAddress = clientAddress;
        this.connection = connection;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        DatagramPacket packet = (DatagramPacket) msg;
        connection.backend.incBytesReceived(packet.content().readableBytes());
        clientChannel.writeAndFlush(new DatagramPacket(packet.content(), clientAddress));
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {

        if (logger.isInfoEnabled()) {
            logger.info("Closing Upstream {} and Downstream {} Channel",
                    connection.clientAddress.getAddress().getHostAddress() + ":" + connection.clientAddress.getPort(),
                    connection.backend.socketAddress().getAddress().getHostAddress() + ":" + connection.backend.socketAddress().getPort());
        }

        connection.connectionActive.set(false); // Mark the Connection as inactive
        ctx.channel().close();                  // Close Downstream Channel
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
