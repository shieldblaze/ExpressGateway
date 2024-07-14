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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http2.Http2Headers;

import java.net.InetSocketAddress;

/**
 * {@link HTTPBalanceRequest} contains {@link InetSocketAddress} and {@link HttpHeaders} of Client
 */
public final class HTTPBalanceRequest extends Request {

    private final HttpHeaders httpHeaders;
    private final Http2Headers http2Headers;

    /**
     * Create a new {@link HTTPBalanceRequest} Instance
     *
     * @param socketAddress {@link InetSocketAddress} of Client
     * @param httpHeaders   {@link HttpHeaders} of Client
     */
    public HTTPBalanceRequest(InetSocketAddress socketAddress, HttpHeaders httpHeaders) {
        this(socketAddress, httpHeaders, null);
    }

    /**
     * Create a new {@link HTTPBalanceRequest} Instance
     *
     * @param socketAddress {@link InetSocketAddress} of Client
     * @param http2Headers  {@link Http2Headers} of Client
     */
    public HTTPBalanceRequest(InetSocketAddress socketAddress, Http2Headers http2Headers) {
        this(socketAddress, null, http2Headers);
    }

    /**
     * Create a new {@link HTTPBalanceRequest} Instance
     *
     * @param socketAddress {@link InetSocketAddress} of Client
     * @param httpHeaders   {@link HttpHeaders} of Client
     * @param http2Headers  {@link Http2Headers} of Client
     */
    public HTTPBalanceRequest(InetSocketAddress socketAddress, HttpHeaders httpHeaders, Http2Headers http2Headers) {
        super(socketAddress);
        this.httpHeaders = httpHeaders;
        this.http2Headers = http2Headers;
    }

    /**
     * Client {@link HttpHeaders}
     */
    public HttpHeaders httpHeaders() {
        return httpHeaders;
    }

    /**
     * Client {@link Http2Headers}
     */
    public Http2Headers http2Headers() {
        return http2Headers;
    }

    @Override
    public String toString() {
        return "HTTPBalanceRequest{" +
                "httpHeaders=" + httpHeaders +
                ", http2Headers=" + http2Headers +
                '}';
    }
}
