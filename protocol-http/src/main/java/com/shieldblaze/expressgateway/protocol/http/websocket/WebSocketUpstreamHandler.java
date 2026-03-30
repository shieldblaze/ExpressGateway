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
import io.netty.handler.codec.http.websocketx.CloseWebSocketFrame;
import io.netty.handler.codec.http.websocketx.PingWebSocketFrame;
import io.netty.handler.codec.http.websocketx.PongWebSocketFrame;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;

public final class WebSocketUpstreamHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(WebSocketUpstreamHandler.class);

    /**
     * WS-F3: Default maximum WebSocket frame payload size in bytes (64 KB).
     *
     * <p>This limit protects the proxy from memory exhaustion attacks where a malicious
     * client sends a single enormous frame. RFC 6455 does not mandate a maximum frame
     * size, but Section 10.4 recommends that implementations limit frame sizes to
     * mitigate denial-of-service. 64 KB is a sensible default that accommodates
     * typical application messages while bounding memory consumption per connection.
     * Nginx uses 64 KB as its default {@code proxy_read_size} for WebSocket as well.</p>
     */
    static final int DEFAULT_MAX_FRAME_PAYLOAD_SIZE = 65536;

    private final Node node;
    private final HTTPLoadBalancer httpLoadBalancer;
    private final WebSocketUpgradeProperty webSocketUpgradeProperty;
    private final int maxFramePayloadSize;
    private WebSocketConnection connection;

    public WebSocketUpstreamHandler(Node node, HTTPLoadBalancer httpLoadBalancer, WebSocketUpgradeProperty webSocketUpgradeProperty) {
        this(node, httpLoadBalancer, webSocketUpgradeProperty, DEFAULT_MAX_FRAME_PAYLOAD_SIZE);
    }

    public WebSocketUpstreamHandler(Node node, HTTPLoadBalancer httpLoadBalancer,
                                    WebSocketUpgradeProperty webSocketUpgradeProperty, int maxFramePayloadSize) {
        this.node = node;
        this.httpLoadBalancer = httpLoadBalancer;
        this.webSocketUpgradeProperty = webSocketUpgradeProperty;
        this.maxFramePayloadSize = maxFramePayloadSize;
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) {
        Bootstrapper bootstrapper = new Bootstrapper(httpLoadBalancer);
        connection = bootstrapper.newInit(node, webSocketUpgradeProperty);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof WebSocketFrame) {
            // WS-06: Handle Ping locally at the proxy — respond with Pong and do NOT
            // forward to backend. This prevents duplicate Pong frames (one from proxy
            // codec, one from backend) reaching the client. Per RFC 6455 Section 5.5.2,
            // a Pong frame sent in response to a Ping MUST have identical Application Data.
            if (msg instanceof PingWebSocketFrame ping) {
                ctx.writeAndFlush(new PongWebSocketFrame(ping.content().retain()));
                ReferenceCountUtil.release(msg);
                return;
            }
            // Drop inbound Pong frames — they are responses to our outbound Pings
            // (if any) and should not be forwarded to the backend.
            if (msg instanceof PongWebSocketFrame) {
                ReferenceCountUtil.release(msg);
                return;
            }

            // WS-F3: Enforce maximum frame payload size to prevent memory exhaustion.
            // RFC 6455 Section 7.4.1 defines status code 1009 (Message Too Big) for
            // this exact scenario: "an endpoint is terminating the connection because
            // it has received a message that is too big for it to process."
            // We check content().readableBytes() which is the payload size of the
            // decoded frame (already past the WebSocket codec).
            WebSocketFrame wsFrame = (WebSocketFrame) msg;
            if (wsFrame.content().readableBytes() > maxFramePayloadSize) {
                logger.warn("WS-F3: Client sent WebSocket frame exceeding max payload size: {} bytes > {} byte limit. "
                        + "Closing connection with 1009 (Message Too Big).",
                        wsFrame.content().readableBytes(), maxFramePayloadSize);
                ReferenceCountUtil.release(msg);
                ctx.writeAndFlush(new CloseWebSocketFrame(1009, "Message Too Big"))
                        .addListener(future -> ctx.close());
                return;
            }

            if (connection != null) {
                connection.writeAndFlush(msg);
                // WS-04: Inline backpressure check — after writing to the backend,
                // if the backend channel is no longer writable, pause reading from
                // the client until the backend drains. We check inline here because
                // channelWritabilityChanged fires on THIS (frontend) channel, not
                // on the backend channel, so it cannot detect backend congestion.
                if (!connection.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
            } else {
                ReferenceCountUtil.release(msg);
            }
        } else {
            ReferenceCountUtil.release(msg);
        }
    }

    /**
     * When the client channel writability changes, this fires on the frontend.
     * If the client has drained and is writable again, re-enable reading from
     * the backend (handled by WebSocketDownstreamHandler). Here we also
     * re-check if the backend is writable to resume client reads that were
     * paused by the inline check in channelRead.
     */
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        if (connection != null && connection.isWritable()) {
            ctx.channel().config().setAutoRead(true);
        }
        super.channelWritabilityChanged(ctx);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught error in WebSocket upstream handler", cause);
        close();
        ctx.close();
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
