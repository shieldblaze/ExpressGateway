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
package com.shieldblaze.expressgateway.core.server.tcp;

import com.shieldblaze.expressgateway.core.loadbalance.backend.Backend;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

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
    private final Backend backend;

    /**
     * Create a {@link DownstreamHandler} for receiving Data from {@link Backend}
     * and send back to {@code Client}.
     *
     * @param clientChannel {@code Client} {@link Channel}
     * @param backend {@link Backend} We'll use this for incrementing {@link Backend#incBytesReceived(int)}
     */
    DownstreamHandler(Channel clientChannel, Backend backend) {
        this.clientChannel = clientChannel;
        this.backend = backend;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        ByteBuf byteBuf = (ByteBuf) msg;                    // Cast Data to ByteBuf
        backend.incBytesReceived(byteBuf.readableBytes());  // Increment number of Bytes Received from Backend
        clientChannel.writeAndFlush(byteBuf);               // Write Data back to Client
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        logger.debug("Closing Client {} and Backend {} Channel", clientChannel, ctx.channel());
        clientChannel.close(); // Close Client Channel
        ctx.channel().close(); // Close Downstream Channel
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
