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

import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.http.websocketx.CloseWebSocketFrame;
import io.netty.handler.timeout.IdleState;
import io.netty.handler.timeout.IdleStateEvent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Handles WebSocket idle timeout events triggered by Netty's
 * {@link io.netty.handler.timeout.IdleStateHandler}.
 *
 * <p>When no frames (read or write) are exchanged for the configured idle duration,
 * this handler initiates a graceful WebSocket close by sending a Close frame with
 * status code 1001 (Going Away) per RFC 6455 Section 7.4.1. The actual close
 * handshake is then handled by {@link WebSocketCloseHandler}.</p>
 *
 * <p>This is analogous to Nginx's {@code proxy_read_timeout} / {@code proxy_send_timeout}
 * for WebSocket connections and HAProxy's {@code timeout tunnel}.</p>
 *
 * <p>Thread safety: This handler is not sharable. Each channel gets its own instance.
 * All events arrive on the channel's EventLoop thread.</p>
 */
final class WebSocketIdleStateHandler extends ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(WebSocketIdleStateHandler.class);

    /**
     * Default idle timeout: 5 minutes (300 seconds).
     */
    static final long DEFAULT_IDLE_TIMEOUT_SECONDS = 300;

    private boolean closeSent;

    @Override
    public void userEventTriggered(ChannelHandlerContext ctx, Object evt) throws Exception {
        if (evt instanceof IdleStateEvent idleEvent) {
            if (idleEvent.state() == IdleState.ALL_IDLE && !closeSent) {
                closeSent = true;
                logger.debug("WebSocket idle timeout, sending Close frame with 1001 (Going Away)");
                // RFC 6455 Section 7.4.1: 1001 indicates the endpoint is going away,
                // such as a server going down or a browser navigating away.
                ctx.writeAndFlush(new CloseWebSocketFrame(1001, "Idle timeout"));
            }
        } else {
            super.userEventTriggered(ctx, evt);
        }
    }
}
