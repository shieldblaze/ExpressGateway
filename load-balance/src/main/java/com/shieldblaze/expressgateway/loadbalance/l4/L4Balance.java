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
package com.shieldblaze.expressgateway.loadbalance.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.loadbalance.Request;
import com.shieldblaze.expressgateway.loadbalance.Response;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;

/**
 * <p> Balance Layer-4 Traffic using the available methods: </p>
 * <ul>
 *     <li> {@link LeastConnection} </li>
 *     <li> {@link Random} </li>
 *     <li> {@link RoundRobin} </li>
 *     <li> {@link WeightedLeastConnection} </li>
 *     <li> {@link WeightedRandom} </li>
 *     <li> {@link WeightedRoundRobin} </li>
 * </ul>
 */
public abstract class L4Balance extends LoadBalance<Backend, Backend, InetSocketAddress, Backend> {

    /**
     * Create {@link L4Balance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Instance
     * @throws NullPointerException If {@link SessionPersistence} is {@code null}
     */
    public L4Balance(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence) {
        super(sessionPersistence);
    }

    public abstract L4Response getResponse(L4Request l4Request);

    @Override
    public Response getResponse(Request request) {
        return getResponse((L4Request) request);
    }
}
