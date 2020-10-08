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

import io.netty.buffer.PooledByteBufAllocator;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;

import static com.shieldblaze.expressgateway.core.server.http.HTTPUtils.setGenericHeaders;

public final class HttpResponses {

    /**
     * HTTP 404: NOT_FOUND
     */
    public static final DefaultFullHttpResponse NOT_FOUND_KEEP_ALIVE = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
            HttpResponseStatus.NOT_FOUND, PooledByteBufAllocator.DEFAULT.buffer());
    public static final DefaultFullHttpResponse NOT_FOUND = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
            HttpResponseStatus.NOT_FOUND, PooledByteBufAllocator.DEFAULT.buffer());

    /**
     * HTTP 500: BAD_GATEWAY
     */
    public static final DefaultFullHttpResponse BAD_GATEWAY_KEEP_ALIVE = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
            HttpResponseStatus.BAD_GATEWAY, PooledByteBufAllocator.DEFAULT.buffer());
    public static final DefaultFullHttpResponse BAD_GATEWAY = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
            HttpResponseStatus.BAD_GATEWAY, PooledByteBufAllocator.DEFAULT.buffer());

    /**
     * HTTP 417: EXPECTATION_FAILED
     */
    public static final DefaultFullHttpResponse EXPECTATION_FAILED = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
            HttpResponseStatus.EXPECTATION_FAILED, PooledByteBufAllocator.DEFAULT.buffer());

    /**
     * HTTP 100: CONTINUE
     */
    public static final DefaultFullHttpResponse ACCEPT_KEEP_ALIVE = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
            HttpResponseStatus.CONTINUE, PooledByteBufAllocator.DEFAULT.buffer());
    public static final DefaultFullHttpResponse ACCEPT = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
            HttpResponseStatus.CONTINUE, PooledByteBufAllocator.DEFAULT.buffer());

    /**
     * HTTP 413: REQUEST_ENTITY_TOO_LARGE
     */
    public static final DefaultFullHttpResponse TOO_LARGE = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
            HttpResponseStatus.REQUEST_ENTITY_TOO_LARGE, PooledByteBufAllocator.DEFAULT.buffer());

    static {
        init();
    }

    public static void init() {
        setKeepAlive(NOT_FOUND_KEEP_ALIVE);
        setClose(NOT_FOUND);

        setKeepAlive(BAD_GATEWAY_KEEP_ALIVE);
        setClose(BAD_GATEWAY);

        setClose(EXPECTATION_FAILED);

        setKeepAlive(ACCEPT_KEEP_ALIVE);
        setClose(ACCEPT);

        setClose(TOO_LARGE);
    }

    static void setKeepAlive(DefaultFullHttpResponse response) {
        response.headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.KEEP_ALIVE);
        response.headers().set(HttpHeaderNames.CONTENT_LENGTH, response.content().readableBytes());
        setGenericHeaders(response.headers());
    }

    static void setClose(DefaultFullHttpResponse response) {
        response.headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);
        response.headers().set(HttpHeaderNames.CONTENT_LENGTH, response.content().readableBytes());
        setGenericHeaders(response.headers());
    }
}
