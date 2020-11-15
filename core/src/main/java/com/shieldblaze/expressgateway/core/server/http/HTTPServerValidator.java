/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.common.utils.ReferenceCounted;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpMessage;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpUtil;
import io.netty.handler.codec.http.HttpVersion;

import static io.netty.handler.codec.http.HttpUtil.getContentLength;

/**
 * Validate "Content-Length" and "Expect" Header
 */
final class HTTPServerValidator extends ChannelInboundHandlerAdapter {

    private final long maxContentLength;

    HTTPServerValidator(HTTPConfiguration httpConfiguration) {
        this.maxContentLength = httpConfiguration.getMaxContentLength();
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;

            if (getContentLength(request, -1L) > maxContentLength) {
                ctx.writeAndFlush(HTTPResponses.TOO_LARGE_413.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                ReferenceCounted.silentRelease(msg);
                return;
            }

            if (isUnsupportedExpectation(request)) {
                ctx.writeAndFlush(HTTPResponses.TOO_LARGE_413.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                ReferenceCounted.silentRelease(msg);
                return;
            }

            if (HttpUtil.is100ContinueExpected(request)) {
                ctx.writeAndFlush(HTTPResponses.ACCEPT_100.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                request.headers().remove(HttpHeaderNames.EXPECT);
            }
        }
        super.channelRead(ctx, msg);
    }

    static boolean isUnsupportedExpectation(HttpMessage message) {
        if (!isExpectHeaderValid(message)) {
            return false;
        }

        String expectValue = message.headers().get(HttpHeaderNames.EXPECT);
        return expectValue != null && !HttpHeaderValues.CONTINUE.toString().equalsIgnoreCase(expectValue);
    }

    private static boolean isExpectHeaderValid(final HttpMessage message) {
        /*
         * Expect: 100-continue is for requests only and it works only on HTTP/1.1 or later. Note further that RFC 7231
         * section 5.1.1 says "A server that receives a 100-continue expectation in an HTTP/1.0 request MUST ignore
         * that expectation."
         */
        return message instanceof HttpRequest && message.protocolVersion().compareTo(HttpVersion.HTTP_1_1) >= 0;
    }
}
