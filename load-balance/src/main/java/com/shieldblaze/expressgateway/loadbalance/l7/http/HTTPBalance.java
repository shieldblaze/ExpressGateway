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
import com.shieldblaze.expressgateway.loadbalance.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.loadbalance.Request;
import com.shieldblaze.expressgateway.loadbalance.Response;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

public abstract class HTTPBalance extends LoadBalance<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Backend> {

    /**
     * Create {@link HTTPBalance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Instance
     * @throws NullPointerException If {@link SessionPersistence} is {@code null}
     */
    public HTTPBalance(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Backend> sessionPersistence) {
        super(sessionPersistence);
    }

    public abstract HTTPBalanceResponse getResponse(HTTPBalanceRequest httpBalanceRequest) throws LoadBalanceException;

    @Override
    public Response getResponse(Request request) throws LoadBalanceException {
        return getResponse((HTTPBalanceRequest) request);
    }
}
