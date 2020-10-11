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
import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.SessionPersistence;

import java.net.InetSocketAddress;
import java.util.List;

/**
 * Select {@link Backend} based on Weight Randomly
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedRandom extends L4Balance {
    private static final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private int totalWeight = 0;

    public WeightedRandom() {
        super(new NOOPSessionPersistence());
    }

    public WeightedRandom(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends);
    }

    public WeightedRandom(SessionPersistence sessionPersistence, List<Backend> backends) {
        super(sessionPersistence);
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
        Backend backend = sessionPersistence.getBackend(sourceAddress);
        if (backend != null) {
            return backend;
        }

        int index = RANDOM_INSTANCE.nextInt(totalWeight);
        backend = backendsMap.get(index);
        sessionPersistence.addRoute(sourceAddress, backend);
        return backend;
    }
}
