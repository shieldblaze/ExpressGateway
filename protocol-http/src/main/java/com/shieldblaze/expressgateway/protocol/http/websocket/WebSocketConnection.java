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
package com.shieldblaze.expressgateway.protocol.http.websocket;

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.common.utils.ReferenceCountedUtil;
import io.netty.channel.ChannelFuture;
import io.netty.handler.codec.http.websocketx.WebSocketClientHandshaker;

/**
 * {@link WebSocketConnection} is a specialized connection type for WebSocket connectivity.
 */
final class WebSocketConnection extends Connection {

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

    private final WebSocketClientHandshaker webSocketClientHandshaker;
    private WebSocketState webSocketState = WebSocketState.INITIATED;

    /**
     * Create a new {@link WebSocketConnection} Instance
     *
     * @param node {@link Node} associated with this Connection
     */
    WebSocketConnection(Node node, WebSocketClientHandshaker webSocketClientHandshaker) {
        super(node);
        this.webSocketClientHandshaker = webSocketClientHandshaker;
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        if (channelFuture.isSuccess()) {
            // Begin handshake
            webSocketClientHandshaker.handshake(channel);

            // Add Listener to handle WebSocket Handshake completion.
            webSocketClientHandshaker.handshakeFuture().addListener(future -> {
                if (future.isSuccess()) {
                    webSocketState = WebSocketState.HANDSHAKE_SUCCESS;
                    writeBacklog();
                } else {
                    webSocketState = WebSocketState.HANDSHAKE_FAILURE;
                    clearBacklog();
                }
            });
        } else {
            clearBacklog();
        }
    }

    @Override
    public void writeAndFlush(Object o) {
        // If Connection State or WebSocket State is `Initiated`, add the data to backlog.
        if (state == State.INITIALIZED || webSocketState == WebSocketState.INITIATED) {
            backlogQueue.add(o);
        } else if (state == State.CONNECTED_AND_ACTIVE && webSocketState == WebSocketState.HANDSHAKE_SUCCESS) {
            channel.writeAndFlush(o, channel.voidPromise());
        } else {
            ReferenceCountedUtil.silentRelease(o);
        }
    }
}
