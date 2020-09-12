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
package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;

import java.net.InetSocketAddress;
import java.util.List;

/**
 * <p> Balance Layer-4 Traffic using the available methods: </p>
 * <ul>
 *     <li> {@link LeastConnection} </li>
 *     <li> {@link Random} </li>
 *     <li> {@link RoundRobin} </li>
 *     <li> {@link SourceIPHash} </li>
 *     <li> {@link WeightedLeastConnection} </li>
 *     <li> {@link WeightedRandom} </li>
 *     <li> {@link WeightedRoundRobin} </li>
 * </ul>
 */
public abstract class L4Balance {
    protected final List<Backend> backends;

    public L4Balance(List<Backend> backends) {
        this.backends = backends;
    }

    public abstract Backend getBackend(InetSocketAddress sourceAddress);
}
