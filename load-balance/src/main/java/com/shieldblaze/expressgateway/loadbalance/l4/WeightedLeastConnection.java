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
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.common.eventstream.EventListener;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.Map;
import java.util.Map.Entry;

/**
 * Select {@link Backend} Based on Weight with Least Connection using Round-Robin
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedLeastConnection extends L4Balance implements EventListener {

    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private final Map<Backend, Integer> localConnectionMap = new HashMap<>();
    private int index = 0;
    private int totalWeight = 0;

    public WeightedLeastConnection() {
        super(new NOOPSessionPersistence());
    }

    public WeightedLeastConnection(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public WeightedLeastConnection(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, Cluster cluster) {
        super(sessionPersistence);
        setCluster(cluster);
    }

    @Override
    public void setCluster(Cluster cluster) {
        super.setCluster(cluster);
        reset();
        cluster.subscribeStream(this);
    }

    private void reset() {
        index = 0;
        totalWeight = 0;

        backendsMap.clear();
        localConnectionMap.clear();
        sessionPersistence.clear();

        cluster.getOnlineBackends().forEach(backend -> {
            backendsMap.put(Range.closed(totalWeight, totalWeight += backend.getWeight()), backend);
            localConnectionMap.put(backend, 0);
        });
    }

    @Override
    public L4Response getResponse(L4Request l4Request) {
        Backend backend = sessionPersistence.getBackend(l4Request);
        if (backend != null) {
            // If Backend is ONLINE then return the response
            // else remove it from session persistence.
            if (backend.getState() == State.ONLINE) {
                return new L4Response(backend);
            } else {
                sessionPersistence.removeRoute(l4Request.getSocketAddress(), backend);
            }
        }

        if (index >= totalWeight) {
            localConnectionMap.replaceAll((b, i) -> i = 0);
            index = 0;
        }

        Entry<Range<Integer>, Backend> backendEntry = backendsMap.getEntry(index);
        index++;
        Integer connections = localConnectionMap.get(backendEntry.getValue());

        if (connections >= backendEntry.getKey().upperEndpoint()) {
            localConnectionMap.put(backendEntry.getValue(), 0);
            index = backendEntry.getKey().upperEndpoint();
        } else {
            localConnectionMap.put(backendEntry.getValue(), connections + 1);
        }

        backend = backendEntry.getValue();
        sessionPersistence.addRoute(l4Request.getSocketAddress(), backend);
        return new L4Response(backend);
    }

    @Override
    public void accept(Object event) {
        if (event instanceof BackendEvent) {
            BackendEvent backendEvent = (BackendEvent) event;
            switch (backendEvent.getType()) {
                case ADDED:
                case ONLINE:
                case OFFLINE:
                case REMOVED:
                   reset();
                default:
                    throw new IllegalArgumentException("Unsupported Backend Event Type: " + backendEvent.getType());
            }
        }
    }
}
