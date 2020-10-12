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
package com.shieldblaze.expressgateway.loadbalance.l7;

import io.netty.handler.codec.http.HttpHeaders;

import java.net.InetSocketAddress;

/**
 * {@link Request} contains {@link InetSocketAddress} and {@link HttpHeaders} of Client
 */
public final class Request {
    private final InetSocketAddress socketAddress;
    private final HttpHeaders httpHeaders;

    /**
     * Create a new {@link Request} Instance
     *
     * @param socketAddress {@link InetSocketAddress} of Client
     * @param httpHeaders   {@link HttpHeaders} of Client
     */
    public Request(InetSocketAddress socketAddress, HttpHeaders httpHeaders) {
        this.socketAddress = socketAddress;
        this.httpHeaders = httpHeaders;
    }

    /**
     * Get Client {@link InetSocketAddress}
     */
    public InetSocketAddress getSocketAddress() {
        return socketAddress;
    }

    /**
     * Get Client {@link HttpHeaders}
     */
    public HttpHeaders getHTTPHeaders() {
        return httpHeaders;
    }
}
