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
package com.shieldblaze.expressgateway.listener.server;

import io.netty.channel.Channel;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;

/**
 * <p> Downstream Handler receives Data from Backend.
 *
 * <p> Flow: </p>
 * <p> &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 * &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 * &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 * &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 *  &nbsp; &nbsp;(Data) </p>
 * (INTERNET) -->-->-->--> (EXPRESSGATEWAY) -->-->-->--> (BACKEND)
 */
final class DownstreamHandler extends ChannelInboundHandlerAdapter {
    /**
     * Client Channel where we'll write all data received from Backend
     */
    private final Channel sourceChannel;

    DownstreamHandler(Channel sourceChannel) {
        this.sourceChannel = sourceChannel;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        sourceChannel.writeAndFlush(msg);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        fullClose(ctx);
    }

    private void fullClose(ChannelHandlerContext ctx) {
        sourceChannel.close();
        ctx.channel().close();
    }
}
