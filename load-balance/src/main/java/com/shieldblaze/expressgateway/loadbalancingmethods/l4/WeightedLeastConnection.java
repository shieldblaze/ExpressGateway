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

import com.google.common.collect.Range;
import com.google.common.collect.TreeRangeMap;
import com.shieldblaze.expressgateway.backend.Backend;
import io.netty.util.internal.ObjectUtil;

import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Map.Entry;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Select {@link Backend} Based on Weight with Least Connection using Round-Robin
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedLeastConnection extends L4Balance {

    private final AtomicInteger Index = new AtomicInteger(0);
    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private final Map<Backend, Integer> localConnectionMap = new HashMap<>();

    private int totalWeight = 0;

    /**
     * Initialize {@link WeightedLeastConnection}
     *
     * @param backendsMap {@link List} of {@link Backend}
     * @throws IllegalArgumentException If {@link List} of {@link Backend} is empty.
     * @throws NullPointerException     If {@link List} of {@link Backend} is {@code null}.
     */
    public WeightedLeastConnection(List<Backend> backendsMap) {
        super(backendsMap);
        ObjectUtil.checkNotNull(backendsMap, "Backend List");
        backendsMap.forEach(backend -> {
            this.backendsMap.put(Range.closed(totalWeight, totalWeight += backend.getWeight()), backend);
            localConnectionMap.put(backend, 0);
        });
        backends.clear();
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        if (Index.get() >= totalWeight) {
            localConnectionMap.replaceAll((b, i) -> i = 0);
            Index.set(0);
        }

        Entry<Range<Integer>, Backend> backend = backendsMap.getEntry(Index.getAndIncrement());
        Integer connections = localConnectionMap.get(backend.getValue());

        if (connections >= backend.getKey().upperEndpoint()) {
            localConnectionMap.put(backend.getValue(), 0);
            Index.set(backend.getKey().upperEndpoint());
        } else {
            localConnectionMap.put(backend.getValue(), connections + 1);
        }

        return backend.getValue();
    }
}
