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

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.common.utils.ReferenceCountedUtil;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.websocketx.WebSocketClientProtocolHandler.ClientHandshakeStateEvent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ConcurrentLinkedQueue;

import static io.netty.handler.codec.http.websocketx.WebSocketClientProtocolHandler.ClientHandshakeStateEvent.HANDSHAKE_COMPLETE;
import static io.netty.handler.codec.http.websocketx.WebSocketClientProtocolHandler.ClientHandshakeStateEvent.HANDSHAKE_TIMEOUT;

/**
 * {@link WebSocketConnection} is a specialized connection type for WebSocket connectivity.
 */
final class WebSocketConnection extends Connection {

    private static final Logger logger = LogManager.getLogger(WebSocketConnection.class);

    private enum WebSocketState {
        /**
         * Connection is initiated but not completed yet.
         */
        INITIATED,

        /**
         * Connection and WebSocket client handshake was successful.
         */
        HANDSHAKE_SUCCESS,

        /**
         * WebSocket client handshake was unsuccessful.
         */
        HANDSHAKE_FAILURE
    }

    private WebSocketState webSocketState = WebSocketState.INITIATED;

    /**
     * Create a new {@link WebSocketConnection} Instance
     *
     * @param node {@link Node} associated with this Connection
     */
    WebSocketConnection(Node node) {
        super(node);
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        if (channelFuture.isSuccess()) {
            ChannelPromise channelPromise = channel.newPromise();
            channel.pipeline().addLast(new WebSocketEventListener(channelPromise));

            // Add Listener to handle WebSocket Handshake completion.
            channelPromise.addListener(future -> {
                if (future.isSuccess()) {
                    webSocketState = WebSocketState.HANDSHAKE_SUCCESS;
                    writeBacklog();
                } else {
                    // MED-09: Backend WebSocket handshake failure -- propagate to client.
                    // Log the failure and close the connection so the client doesn't hang.
                    webSocketState = WebSocketState.HANDSHAKE_FAILURE;
                    logger.warn("Backend WebSocket handshake failed", future.cause());
                    clearBacklog();
                    close();
                }
            });
        } else {
            clearBacklog();
        }
    }

    /**
     * WS-F1: Adds the same backlogSize bound check that the parent {@link Connection#writeAndFlush(Object)}
     * enforces. Without this guard, a fast client sending during the WebSocket handshake phase could
     * grow the backlog queue without bound, leading to OOM.
     */
    @Override
    public void writeAndFlush(Object o) {
        // If Connection State or WebSocket State is `Initiated`, add the data to backlog.
        if (state == State.INITIALIZED || webSocketState == WebSocketState.INITIATED) {
            ConcurrentLinkedQueue<Object> queue = backlogQueue;
            if (queue != null) {
                // Increment-then-check: atomic, prevents concurrent threads from
                // both passing the size check and exceeding the limit.
                int currentSize = backlogSize.incrementAndGet();
                if (currentSize > maxBacklogSize) {
                    backlogSize.decrementAndGet();
                    ReferenceCountedUtil.silentRelease(o);
                    throw new IllegalStateException("WebSocket backlog queue capacity exceeded");
                }
                queue.add(o);
            } else {
                ReferenceCountedUtil.silentRelease(o);
            }
        } else if (state == State.CONNECTED_AND_ACTIVE && webSocketState == WebSocketState.HANDSHAKE_SUCCESS) {
            channel.writeAndFlush(o);
        } else {
            ReferenceCountedUtil.silentRelease(o);
        }
    }

    /**
     * Check if the underlying channel is writable for backpressure signaling.
     *
     * @return {@code true} if the channel is connected and writable, {@code false} otherwise
     */
    boolean isWritable() {
        return channel != null && channel.isWritable();
    }

    private static final class WebSocketEventListener extends ChannelInboundHandlerAdapter {

        private final ChannelPromise channelPromise;

        WebSocketEventListener(ChannelPromise channelPromise) {
            this.channelPromise = channelPromise;
        }

        @Override
        public void userEventTriggered(ChannelHandlerContext ctx, Object evt) throws Exception {
            if (evt instanceof ClientHandshakeStateEvent event) {
                if (event == HANDSHAKE_COMPLETE) {
                    channelPromise.setSuccess();
                    ctx.pipeline().remove(this);
                } else if (event == HANDSHAKE_TIMEOUT) {
                    channelPromise.setFailure(new IllegalStateException("WebSocket Handshake Failed, Event: HANDSHAKE_TIMEOUT"));
                    ctx.pipeline().remove(this);
                }
            }
        }
    }
}
