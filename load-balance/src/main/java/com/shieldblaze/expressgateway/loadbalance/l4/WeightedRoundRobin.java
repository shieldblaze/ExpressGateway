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

import com.google.common.collect.Range;
import com.google.common.collect.TreeRangeMap;
import com.shieldblaze.expressgateway.loadbalance.backend.Backend;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Select {@link Backend} based on Weight using Round-Robin
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedRoundRobin extends L4Balance {

    private final AtomicInteger Index = new AtomicInteger();
    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private int totalWeight = 0;

    public WeightedRoundRobin() {
    }

    public WeightedRoundRobin(List<Backend> backends) {
        setBackends(backends);
    }

    @Override
    public void setBackends(List<Backend> backends) {
        super.setBackends(backends);
        this.backends.forEach(backend -> this.backendsMap.put(Range.closed(totalWeight, totalWeight += backend.getWeight()), backend));
        backends.clear();
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        if (Index.get() >= totalWeight) {
            Index.set(0);
        }
        return backendsMap.get(Index.getAndIncrement());
    }
}
