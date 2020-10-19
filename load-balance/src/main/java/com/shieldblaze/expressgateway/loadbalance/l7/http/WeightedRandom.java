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
package com.shieldblaze.expressgateway.loadbalance.l7.http;

import com.google.common.collect.Range;
import com.google.common.collect.TreeRangeMap;
import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.common.eventstream.EventListener;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

/**
 * Select {@link Backend} based on Weight Randomly
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedRandom extends HTTPBalance implements EventListener {
    private final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private int totalWeight = 0;

    public WeightedRandom() {
        super(new NOOPSessionPersistence());
    }

    public WeightedRandom(Cluster cluster) {
        this(new NOOPSessionPersistence(), cluster);
    }

    public WeightedRandom(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Backend> sessionPersistence, Cluster cluster) {
        super(sessionPersistence);
        setCluster(cluster);
    }

    @Override
    public void setCluster(Cluster cluster) {
        super.setCluster(cluster);
        reset();
    }

    private void reset() {
        cluster.getOnlineBackends().forEach(backend -> this.backendsMap.put(Range.closed(totalWeight, totalWeight += backend.getWeight()), backend));
    }

    @Override
    public HTTPBalanceResponse getResponse(HTTPBalanceRequest httpBalanceRequest) {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.getBackend(httpBalanceRequest);
        if (httpBalanceResponse != null) {
            return httpBalanceResponse;
        }

        int index = RANDOM_INSTANCE.nextInt(totalWeight);
        Backend backend = backendsMap.get(index);
        return sessionPersistence.addRoute(httpBalanceRequest, backend);
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
