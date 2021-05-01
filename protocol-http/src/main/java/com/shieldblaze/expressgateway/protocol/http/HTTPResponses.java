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
package com.shieldblaze.expressgateway.protocol.http;

import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;

import static io.netty.handler.codec.http.HttpResponseStatus.BAD_GATEWAY;
import static io.netty.handler.codec.http.HttpResponseStatus.CONTINUE;
import static io.netty.handler.codec.http.HttpResponseStatus.EXPECTATION_FAILED;
import static io.netty.handler.codec.http.HttpResponseStatus.HTTP_VERSION_NOT_SUPPORTED;
import static io.netty.handler.codec.http.HttpResponseStatus.NOT_FOUND;
import static io.netty.handler.codec.http.HttpResponseStatus.REQUEST_ENTITY_TOO_LARGE;
import static io.netty.handler.codec.http.HttpResponseStatus.SERVICE_UNAVAILABLE;

/**
 * {@link HTTPResponses} contains generic HTTP Responses.
 */
public final class HTTPResponses {

    /**
     * HTTP 100: CONTINUE
     */
    public static final FullHttpResponse ACCEPT_100 = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, CONTINUE);

    /**
     * HTTP 400: BAD_REQUEST
     */
    public static final FullHttpResponse BAD_REQUEST_400 = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.NOT_FOUND);

    /**
     * HTTP 404: NOT_FOUND
     */
    public static final FullHttpResponse NOT_FOUND_404 = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, NOT_FOUND);

    /**
     * HTTP 417: EXPECTATION_FAILED
     */
    public static final FullHttpResponse EXPECTATION_FAILED_417 = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, EXPECTATION_FAILED);

    /**
     * HTTP 413: REQUEST_ENTITY_TOO_LARGE
     */
    public static final FullHttpResponse TOO_LARGE_413 = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, REQUEST_ENTITY_TOO_LARGE);

    /**
     * HTTP 502: BAD_GATEWAY
     */
    public static final FullHttpResponse BAD_GATEWAY_502 = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, BAD_GATEWAY);

    /**
     * HTTP 503: SERVICE_UNAVAILABLE
     */
    public static final FullHttpResponse SERVICE_UNAVAILABLE_503 = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, SERVICE_UNAVAILABLE);

    /**
     * HTTP 505: HTTP_VERSION_NOT_SUPPORTED
     */
    public static final FullHttpResponse HTTP_VERSION_NOT_SUPPORTED_505 = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HTTP_VERSION_NOT_SUPPORTED);

    static {
        init();
    }

    public static void init() {
        setContentLength(ACCEPT_100);
        setContentLength(BAD_REQUEST_400);
        setContentLength(NOT_FOUND_404);
        setContentLength(EXPECTATION_FAILED_417);
        setContentLength(TOO_LARGE_413);
        setContentLength(BAD_GATEWAY_502);
        setContentLength(SERVICE_UNAVAILABLE_503);
    }

    private static void setContentLength(FullHttpResponse response) {
        response.headers().set(HttpHeaderNames.CONTENT_LENGTH, response.content().readableBytes());
    }
}
