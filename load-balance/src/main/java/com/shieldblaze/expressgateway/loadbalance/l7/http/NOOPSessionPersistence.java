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
package com.shieldblaze.expressgateway.loadbalance.l7.http;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.Request;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;
import io.netty.handler.codec.http.EmptyHttpHeaders;

/**
 * No-Operation {@link SessionPersistence}
 */
final class NOOPSessionPersistence implements SessionPersistence<HTTPResponse, HTTPResponse, HTTPRequest, Backend> {

    @Override
    public HTTPResponse getBackend(Request request) {
        return null;
    }

    @Override
    public HTTPResponse addRoute(HTTPRequest httpRequest, Backend backend) {
        return new HTTPResponse(backend, EmptyHttpHeaders.INSTANCE);
    }
}
