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
import io.netty.handler.codec.http.websocketx.CloseWebSocketFrame;
import io.netty.handler.codec.http.websocketx.PingWebSocketFrame;
import io.netty.handler.codec.http.websocketx.PongWebSocketFrame;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;

final class WebSocketDownstreamHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(WebSocketDownstreamHandler.class);

    // HIGH-02: volatile for safe cross-thread visibility — close() may null this
    // from a different thread than channelWritabilityChanged() / channelRead().
    private volatile Channel upstreamChannel;
    private final int maxFramePayloadSize;

    @NonNull
    WebSocketDownstreamHandler(Channel upstreamChannel) {
        this(upstreamChannel, 65536);
    }

    @NonNull
    WebSocketDownstreamHandler(Channel upstreamChannel, int maxFramePayloadSize) {
        this.upstreamChannel = upstreamChannel;
        this.maxFramePayloadSize = maxFramePayloadSize;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof WebSocketFrame wsFrame) {
            // MED-16: Handle Ping/Pong locally — don't forward to client.
            if (msg instanceof PingWebSocketFrame ping) {
                ctx.writeAndFlush(new PongWebSocketFrame(ping.content().retain()));
                ReferenceCountUtil.release(msg);
                return;
            }
            if (msg instanceof PongWebSocketFrame) {
                ReferenceCountUtil.release(msg);
                return;
            }

            // HIGH-02: Local capture to avoid NPE if close() nulls upstreamChannel concurrently.
            Channel ch = this.upstreamChannel;

            // HIGH-12: Enforce frame size limit on downstream (backend) frames.
            if (wsFrame.content().readableBytes() > maxFramePayloadSize) {
                logger.warn("HIGH-12: Backend sent WebSocket frame exceeding max payload size: {} > {} bytes",
                        wsFrame.content().readableBytes(), maxFramePayloadSize);
                ReferenceCountUtil.release(msg);
                if (ch != null) {
                    ch.writeAndFlush(new CloseWebSocketFrame(1009, "Message Too Big"));
                }
                ctx.close();
                return;
            }

            if (ch != null && ch.isActive()) {
                ch.writeAndFlush(msg);
            } else {
                // HIGH-05: Release the frame if upstream channel is gone.
                ReferenceCountUtil.release(msg);
            }
        } else {
            ReferenceCountUtil.release(msg);
        }
    }

    // WS-04: Backpressure — when the upstream (client-facing) channel is not writable,
    // pause reading from the backend to avoid unbounded buffering in the proxy.
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        // HIGH-02: Local capture to eliminate TOCTOU race — close() can null
        // the volatile field between the null-check and isWritable() call.
        Channel ch = this.upstreamChannel;
        if (ch != null) {
            boolean writable = ch.isWritable();
            ctx.channel().config().setAutoRead(writable);
        }
        super.channelWritabilityChanged(ctx);
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
    public synchronized void close() {
        // HIGH-05: Capture and null atomically to prevent double-close.
        Channel ch = this.upstreamChannel;
        if (ch != null) {
            this.upstreamChannel = null;
            ch.close();
        }
    }
}
