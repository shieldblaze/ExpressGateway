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
package com.shieldblaze.expressgateway.protocol.http;

import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.nio.charset.StandardCharsets;

import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_LENGTH;
import static io.netty.handler.codec.http.HttpHeaderNames.CONNECTION;
import static io.netty.handler.codec.http.HttpVersion.HTTP_1_1;

/**
 * Enforces a maximum request body size limit.
 * <p>
 * Per RFC 9110 Section 15.5.14, a server SHOULD respond with 413 (Content Too Large)
 * when the request content is larger than the server is willing to process. This handler
 * tracks the accumulated size of {@link HttpContent} messages for each request and rejects
 * the request with a 413 response if the limit is exceeded.
 * </p>
 * <p>
 * This is analogous to Nginx's {@code client_max_body_size} directive and protects against
 * clients sending excessively large payloads that could exhaust memory or storage.
 * </p>
 *
 * <p>Thread safety: This handler is not sharable. Each channel gets its own instance.
 * All reads for a given channel occur on the same EventLoop thread, so no synchronization
 * is needed for the {@code accumulatedSize} and {@code exceeded} fields.</p>
 */
public class RequestBodySizeLimitHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(RequestBodySizeLimitHandler.class);

    /**
     * Default maximum body size: 10 MB.
     */
    public static final long DEFAULT_MAX_BODY_SIZE = 10L * 1024 * 1024;

    private final long maxBodySize;
    private long accumulatedSize;
    private boolean exceeded;

    /**
     * Creates a handler with the default maximum body size of 10 MB.
     */
    public RequestBodySizeLimitHandler() {
        this(DEFAULT_MAX_BODY_SIZE);
    }

    /**
     * Creates a handler with a custom maximum body size.
     *
     * @param maxBodySize Maximum allowed request body size in bytes. Must be positive.
     */
    public RequestBodySizeLimitHandler(long maxBodySize) {
        if (maxBodySize <= 0) {
            throw new IllegalArgumentException("maxBodySize must be positive: " + maxBodySize);
        }
        this.maxBodySize = maxBodySize;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest) {
            // New request: reset accumulated size and exceeded flag
            accumulatedSize = 0;
            exceeded = false;
        }

        if (msg instanceof HttpContent httpContent) {
            if (exceeded) {
                // Already rejected this request; discard subsequent chunks silently
                ReferenceCountUtil.release(msg);
                return;
            }

            accumulatedSize += httpContent.content().readableBytes();

            if (accumulatedSize > maxBodySize) {
                exceeded = true;
                ReferenceCountUtil.release(msg);

                logger.warn("Request body size {} exceeds limit {} bytes, responding with 413",
                        accumulatedSize, maxBodySize);

                // H1-08: Stop reading more data from the client immediately.
                // Without this, Netty continues to read and buffer inbound data
                // even after we've decided to reject the request, wasting memory
                // and network bandwidth.
                ctx.channel().config().setAutoRead(false);

                // RFC 9110 Section 15.5.14: 413 Content Too Large
                byte[] body = "Request body too large".getBytes(StandardCharsets.UTF_8);
                FullHttpResponse response = new DefaultFullHttpResponse(
                        HTTP_1_1,
                        HttpResponseStatus.REQUEST_ENTITY_TOO_LARGE,
                        Unpooled.wrappedBuffer(body)
                );
                response.headers().set(CONTENT_LENGTH, body.length);
                response.headers().set(CONNECTION, "close");

                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }
        }

        // Pass through: either an HttpRequest (headers only) or an HttpContent within limits
        super.channelRead(ctx, msg);
    }
}
