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
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.common.eventstream.EventListener;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;

/**
 * Select {@link Backend} based on Weight using Round-Robin
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedRoundRobin extends L4Balance implements EventListener {

    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private int index = 0;
    private int totalWeight = 0;

    public WeightedRoundRobin() {
        super(new NOOPSessionPersistence());
    }

    public WeightedRoundRobin(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public WeightedRoundRobin(SessionPersistence<Backend, Backend, InetSocketAddress, Backend> sessionPersistence, Cluster cluster) {
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
        cluster.getOnlineBackends().forEach(backend -> this.backendsMap.put(Range.closed(totalWeight, totalWeight += backend.getWeight()), backend));
    }

    @Override
    public L4Response getResponse(L4Request l4Request) {
        Backend backend = sessionPersistence.getBackend(l4Request);
        if (backend != null) {
            return new L4Response(backend);
        }

        if (index >= totalWeight) {
            index = 0;
        }

        backend = backendsMap.get(index);
        index++;
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
