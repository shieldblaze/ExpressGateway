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

import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.LastHttpContent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

import static io.netty.handler.codec.http.HttpHeaderNames.CONNECTION;
import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_LENGTH;
import static io.netty.handler.codec.http.HttpResponseStatus.REQUEST_TIMEOUT;
import static io.netty.handler.codec.http.HttpVersion.HTTP_1_1;

/**
 * ME-04: Slow-POST defense — enforces a maximum time for receiving the complete
 * request body after headers arrive.
 *
 * <h3>Problem</h3>
 * <p>A slow-POST attacker sends request headers normally, then trickles the body
 * at 1 byte/second. The existing {@code ConnectionTimeoutHandler} resets its idle
 * timer on every {@code channelRead()}, so a trickle of data keeps the connection
 * alive indefinitely. This ties up backend connections, file descriptors, and memory.</p>
 *
 * <h3>Solution</h3>
 * <p>When an {@link HttpRequest} that has a body (is not a {@link FullHttpRequest})
 * arrives, this handler starts a one-shot deadline timer. If the complete body
 * ({@link LastHttpContent}) is not received within the configured timeout, the
 * connection is closed with a 408 Request Timeout response.</p>
 *
 * <p>For requests without a body (GET, HEAD, DELETE sent as FullHttpRequest),
 * no timer is started — there is nothing to trickle.</p>
 *
 * <h3>Thread safety</h3>
 * <p>Not {@code @Sharable}. All access is confined to the channel's EventLoop thread.</p>
 *
 * <p>Analogous to Apache httpd's {@code RequestReadTimeout body=N} directive.</p>
 */
public final class RequestBodyTimeoutHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(RequestBodyTimeoutHandler.class);

    private final long timeoutSeconds;
    private ScheduledFuture<?> bodyTimeout;

    public RequestBodyTimeoutHandler(long timeoutSeconds) {
        this.timeoutSeconds = timeoutSeconds;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest && !(msg instanceof FullHttpRequest)) {
            // Request with body (not complete in one message) — start deadline
            startTimeout(ctx);
        }

        if (msg instanceof LastHttpContent) {
            // Body complete — cancel the deadline
            cancelTimeout();
        }

        super.channelRead(ctx, msg);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        cancelTimeout();
        super.channelInactive(ctx);
    }

    @Override
    public void handlerRemoved(ChannelHandlerContext ctx) throws Exception {
        cancelTimeout();
    }

    private void startTimeout(ChannelHandlerContext ctx) {
        cancelTimeout();
        bodyTimeout = ctx.executor().schedule(() -> {
            if (ctx.channel().isActive()) {
                logger.warn("ME-04: Request body timeout ({}s) expired for {} — possible slow-POST attack. Closing.",
                        timeoutSeconds, ctx.channel().remoteAddress());

                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, REQUEST_TIMEOUT);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(CONNECTION, "close");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
            }
        }, timeoutSeconds, TimeUnit.SECONDS);
    }

    private void cancelTimeout() {
        if (bodyTimeout != null) {
            bodyTimeout.cancel(false);
            bodyTimeout = null;
        }
    }
}
