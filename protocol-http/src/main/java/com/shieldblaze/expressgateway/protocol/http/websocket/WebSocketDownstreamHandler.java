/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.http.websocket;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;

final class WebSocketDownstreamHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(WebSocketDownstreamHandler.class);

    private Channel upstreamChannel;

    @NonNull
    WebSocketDownstreamHandler(Channel upstreamChannel) {
        this.upstreamChannel = upstreamChannel;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        // If received message is WebSocketFrame then write it back to the client else release it.
        if (msg instanceof WebSocketFrame) {
            upstreamChannel.writeAndFlush(msg);
        } else {
            ReferenceCountUtil.release(msg);
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        close();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        try {
            if (cause instanceof IOException) {
                // Swallow this harmless IOException
            } else {
                logger.error("Caught error, Closing connections", cause);
            }
        } finally {
            close();
        }
    }

    @Override
    public void close() {
        if (upstreamChannel != null) {
            upstreamChannel.close();
            upstreamChannel = null;
        }
    }
}
