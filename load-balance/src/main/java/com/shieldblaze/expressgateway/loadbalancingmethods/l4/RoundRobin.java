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
import io.netty.util.internal.ObjectUtil;

import java.net.InetSocketAddress;
import java.util.List;

/**
 * Select {@link Backend} based on Round-Robin
 */
public final class RoundRobin extends L4Balance {

    private final RoundRobinImpl<Backend> backendsRoundRobin;

    /**
     * Initialize {@link RoundRobin}
     *
     * @param backends {@link List} of {@link Backend}
     * @throws IllegalArgumentException If {@link List} of {@link Backend} cannot be empty.
     * @throws NullPointerException     If {@link List} of {@link Backend} is {@code null}.
     */
    public RoundRobin(List<Backend> backends) {
        super(backends);
        ObjectUtil.checkNotNull(backends, "Backend List");
        backendsRoundRobin = new RoundRobinImpl<>(this.backends);
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        return backendsRoundRobin.iterator().next();
    }
}
