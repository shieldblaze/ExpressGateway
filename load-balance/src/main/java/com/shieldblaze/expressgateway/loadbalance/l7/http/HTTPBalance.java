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
import com.shieldblaze.expressgateway.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.loadbalance.Request;
import com.shieldblaze.expressgateway.loadbalance.Response;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.util.List;

public abstract class HTTPL7Balance extends LoadBalance<Backend, HTTPRequest, HTTPResponse> {

    /**
     * Create {@link HTTPL7Balance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Instance
     * @throws NullPointerException If {@link SessionPersistence} is {@code null}
     */
    public HTTPL7Balance(SessionPersistence<Backend, HTTPRequest, HTTPResponse> sessionPersistence) {
        super(sessionPersistence);
    }

    public abstract HTTPResponse getBackend(HTTPRequest httpRequest);

    @Override
    public Response getResponse(Request request) {
        return getBackend((HTTPRequest) request);
    }
}
