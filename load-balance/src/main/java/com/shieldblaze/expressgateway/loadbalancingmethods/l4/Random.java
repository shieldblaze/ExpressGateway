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
 * Select Backend Randomly
 */
public final class Random extends L4Balance {
    private static final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    public Random(List<Backend> socketAddressList) {
        super(socketAddressList);
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        int index = RANDOM_INSTANCE.nextInt(getBackends().size());
        return getBackends().get(index);
    }
}
