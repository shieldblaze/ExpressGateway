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
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;

public abstract class HTTPBalance extends LoadBalance<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> {

    /**
     * Create {@link LoadBalance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    protected HTTPBalance(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence) {
        super(sessionPersistence);
    }

    public abstract HTTPBalanceResponse response(HTTPBalanceRequest httpBalanceRequest) throws LoadBalanceException;

    @Override
    public Response response(Request request) throws LoadBalanceException {
        return response((HTTPBalanceRequest) request);
    }

    @Override
    public String toString() {
        return "HTTPBalance{" +
                "sessionPersistence=" + sessionPersistence +
                ", cluster=" + cluster +
                '}';
    }
}
