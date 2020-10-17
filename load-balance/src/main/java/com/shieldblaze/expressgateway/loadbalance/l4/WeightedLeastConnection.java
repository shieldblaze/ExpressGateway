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
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Map.Entry;

/**
 * Select {@link Backend} Based on Weight with Least Connection using Round-Robin
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedLeastConnection extends L4Balance {

    private int index = 0;
    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private final Map<Backend, Integer> localConnectionMap = new HashMap<>();
    private int totalWeight = 0;

    public WeightedLeastConnection() {
        super(new NOOPSessionPersistence());
    }

    public WeightedLeastConnection(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends);
    }

    public WeightedLeastConnection(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, List<Backend> backends) {
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
    public L4Response getResponse(L4Request l4Request) {
        Backend _backend = sessionPersistence.getBackend(new L4Request(l4Request.getSocketAddress()));
        if (_backend != null) {
            return new L4Response(_backend);
        }

        if (index >= totalWeight) {
            localConnectionMap.replaceAll((backend, i) -> i = 0);
            index = 0;
        }

        Entry<Range<Integer>, Backend> backend = backendsMap.getEntry(index);
        index++;
        Integer connections = localConnectionMap.get(backend.getValue());

        if (connections >= backend.getKey().upperEndpoint()) {
            localConnectionMap.put(backend.getValue(), 0);
            index = backend.getKey().upperEndpoint();
        } else {
            localConnectionMap.put(backend.getValue(), connections + 1);
        }

        _backend = backend.getValue();
        sessionPersistence.addRoute(l4Request.getSocketAddress(), _backend);
        return new L4Response(_backend);
    }
}
