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

import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.l4.sessionpersistence.SessionPersistence;
import io.netty.util.internal.ObjectUtil;

import java.net.InetSocketAddress;
import java.util.ArrayList;
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
    protected SessionPersistence sessionPersistence;
    protected List<Backend> backends;

    /**
     * Create {@link L4Balance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Instance
     * @throws NullPointerException If {@link SessionPersistence} is {@code null}
     */
    public L4Balance(SessionPersistence sessionPersistence) {
        this.sessionPersistence = ObjectUtil.checkNotNull(sessionPersistence, "Session Persistence");
    }

    /**
     * Set Backends
     *
     * @param backends {@link List} of {@link Backend}
     * @throws IllegalArgumentException If {@link List} of {@link Backend} is Empty.
     * @throws NullPointerException     If {@link List} of {@link Backend} is {@code null}.
     */
    public void setBackends(List<Backend> backends) {
        ObjectUtil.checkNotNull(backends, "Backend List");
        if (backends.size() == 0) {
            throw new IllegalArgumentException("Backends List Cannot Be Empty");
        }
        this.backends = new ArrayList<>(backends);
    }

    public abstract Backend getBackend(InetSocketAddress sourceAddress);
}
