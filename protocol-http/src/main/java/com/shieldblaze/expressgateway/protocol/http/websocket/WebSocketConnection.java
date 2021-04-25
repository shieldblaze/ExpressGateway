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
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.websocketx.WebSocketClientHandshaker;

final class WebSocketConnection extends Connection {

    private final WebSocketClientHandshaker webSocketClientHandshaker;
    private final ChannelPromise channelPromise;

    /**
     * Create a new {@link WebSocketConnection} Instance
     *
     * @param node {@link Node} associated with this Connection
     */
    WebSocketConnection(Node node, WebSocketClientHandshaker webSocketClientHandshaker, ChannelPromise channelPromise) {
        super(node);
        this.webSocketClientHandshaker = webSocketClientHandshaker;
        this.channelPromise = channelPromise;
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        if (channelFuture.isSuccess()) {
            webSocketClientHandshaker.handshake(channelFuture.channel()).addListener(future -> {
                if (future.isSuccess()) {
                    channelPromise.setSuccess(null);
                    writeBacklog();
                } else {
                    channelPromise.setFailure(future.cause());
                    clearBacklog();
                }
            });
        } else {
            clearBacklog();
        }
    }
}
