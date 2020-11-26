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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.backend.Node;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final Channel upstream;
    private final Node node;
    private final InetSocketAddress upstreamAddress;

    /**
     * Create a {@link DownstreamHandler} for receiving Data from {@link Node}
     * and send back to {@code Client}.
     *
     * @param upstream {@code Client} {@link Channel}
     * @param node  {@link Node} We'll use this for incrementing {@link Node#incBytesReceived(int)}
     */
    DownstreamHandler(Channel upstream, Node node) {
        this.upstream = upstream;
        this.node = node;
        this.upstreamAddress = (InetSocketAddress) upstream.remoteAddress();
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        ByteBuf byteBuf = (ByteBuf) msg;                    // Cast Data to ByteBuf
        node.incBytesReceived(byteBuf.readableBytes());     // Increment number of Bytes Received from Backend
        upstream.writeAndFlush(byteBuf);                    // Write Data back to Client
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (logger.isInfoEnabled()) {
            logger.info("Closing Upstream {} and Downstream {} Channel",
                    upstreamAddress.getAddress().getHostAddress() + ":" + upstreamAddress.getPort(),
                    node.socketAddress().getAddress().getHostAddress() + ":" + node.socketAddress().getPort());
        }
        upstream.close();      // Close Upstream Channel
        ctx.channel().close(); // Close Downstream Channel
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
