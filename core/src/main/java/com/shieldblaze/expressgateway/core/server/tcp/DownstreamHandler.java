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

import com.shieldblaze.expressgateway.backend.Backend;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
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

    private final Channel upstream;
    private final Backend backend;
    private final InetSocketAddress upstreamAddress;

    /**
     * Create a {@link DownstreamHandler} for receiving Data from {@link Backend}
     * and send back to {@code Client}.
     *
     * @param upstream {@code Client} {@link Channel}
     * @param backend  {@link Backend} We'll use this for incrementing {@link Backend#incBytesReceived(int)}
     */
    DownstreamHandler(Channel upstream, Backend backend) {
        this.upstream = upstream;
        this.backend = backend;
        this.upstreamAddress = (InetSocketAddress) upstream.remoteAddress();
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        ByteBuf byteBuf = (ByteBuf) msg;                    // Cast Data to ByteBuf
        backend.incBytesReceived(byteBuf.readableBytes());  // Increment number of Bytes Received from Backend
        upstream.writeAndFlush(byteBuf);                    // Write Data back to Client
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (logger.isInfoEnabled()) {
            logger.info("Closing Upstream {} and Downstream {} Channel",
                    upstreamAddress.getAddress().getHostAddress() + ":" + upstreamAddress.getPort(),
                    backend.getSocketAddress().getAddress().getHostAddress() + ":" + backend.getSocketAddress().getPort());
        }
        upstream.close();      // Close Upstream Channel
        ctx.channel().close(); // Close Downstream Channel
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
