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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.util.ReferenceCountUtil;

import java.io.Closeable;

public final class WebSocketUpstreamHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private final Node node;
    private final HTTPLoadBalancer httpLoadBalancer;
    private final WebSocketUpgradeProperty webSocketUpgradeProperty;
    private WebSocketConnection connection;

    public WebSocketUpstreamHandler(Node node, HTTPLoadBalancer httpLoadBalancer, WebSocketUpgradeProperty webSocketUpgradeProperty) {
        this.node = node;
        this.httpLoadBalancer = httpLoadBalancer;
        this.webSocketUpgradeProperty = webSocketUpgradeProperty;
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) {
        Bootstrapper bootstrapper = new Bootstrapper(httpLoadBalancer);
        connection = bootstrapper.newInit(node, webSocketUpgradeProperty);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        // If received message is WebSocketFrame then write it back
        // to the client else release it.
        if (msg instanceof WebSocketFrame) {
            connection.writeAndFlush(msg);
        } else {
            ReferenceCountUtil.release(msg);
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        cause.printStackTrace();
    }

    @Override
    public void handlerRemoved(ChannelHandlerContext ctx) {
        close();
    }

    @Override
    public void close() {
        if (connection != null) {
            connection.close();
            connection = null;
        }
    }
}
