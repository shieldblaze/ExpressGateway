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

import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelPipeline;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.websocketx.WebSocketClientHandshaker;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * This class completes WebSocket client handshake. Once handshake is done,
 * this handler removes itself from {@link ChannelPipeline}.
 */
final class WebSocketClientHandshakerFinisherHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(WebSocketClientHandshakerFinisherHandler.class);

    private final WebSocketClientHandshaker handshaker;

    WebSocketClientHandshakerFinisherHandler(WebSocketClientHandshaker handshaker) {
        this.handshaker = handshaker;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        logger.debug("WebSocket handshaker received: {}", msg);

        // If Message is FullHttpResponse and handshake is incomplete
        // then capture the FullHttpResponse and pass it to handshaker
        // to complete the handshake.
        if (msg instanceof FullHttpResponse response && !handshaker.isHandshakeComplete()) {
            logger.debug("Completing WebSocket client handshake");
            handshaker.finishHandshake(ctx.channel(), response); // Finish the handshake
            response.release();          // Release the HttpResponse
            ctx.pipeline().remove(this); // Remove ourselves — handshake is done.
            return;
        }

        ctx.fireChannelRead(msg);
    }

    @Override
    public void userEventTriggered(ChannelHandlerContext ctx, Object evt) throws Exception {
        logger.debug("WebSocket handshaker user event: {}", evt);
        super.userEventTriggered(ctx, evt);
    }
}
