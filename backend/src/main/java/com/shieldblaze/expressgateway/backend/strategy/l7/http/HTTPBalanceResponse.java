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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import io.netty.handler.codec.http.HttpHeaders;

/**
 * {@link HTTPBalanceResponse} contains selected {@link Node} and {@link HttpHeaders} for response.
 */
public class HTTPBalanceResponse extends Response {
    private final HttpHeaders httpHeaders;

    /**
     * Create a {@link HTTPBalanceResponse} Instance
     *
     * @param node     Selected {@linkplain Node} for the request
     * @param httpHeaders {@linkplain HttpHeaders} for response
     */
    public HTTPBalanceResponse(Node node, HttpHeaders httpHeaders) {
        super(node);
        this.httpHeaders = httpHeaders;
    }

    /**
     * Get {@link HttpHeaders} for {@link HTTPBalanceRequest} response.
     */
    public HttpHeaders getHTTPHeaders() {
        return httpHeaders;
    }

    @Override
    public String toString() {
        return "HTTPResponse{httpHeaders=" + httpHeaders + ", backend=" + node() + '}';
    }
}
