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

    public WeightedLeastConnection() {
        super(new NOOPSessionPersistence());
    }

    public WeightedLeastConnection(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends);
    }

    public WeightedLeastConnection(SessionPersistence sessionPersistence, List<Backend> backends) {
        super(sessionPersistence);
        setBackends(backends);
    }

    @Override
    public void setBackends(List<Backend> backends) {
        super.setBackends(backends);
        this.backends.forEach(backend -> {
            this.backendsMap.put(Range.closed(totalWeight, totalWeight += backend.getWeight()), backend);
            localConnectionMap.put(backend, 0);
        });
        backends.clear();
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        Backend _backend = sessionPersistence.getBackend(sourceAddress);
        if (_backend != null) {
            return _backend;
        }

        if (Index.get() >= totalWeight) {
            localConnectionMap.replaceAll((backend, i) -> i = 0);
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

        _backend = backend.getValue();
        sessionPersistence.addRoute(sourceAddress, _backend);
        return _backend;
    }
}
