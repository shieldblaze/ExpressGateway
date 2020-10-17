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
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;

import java.util.List;

/**
 * Select {@link Backend} based on Weight using Round-Robin
 */
@SuppressWarnings("UnstableApiUsage")
public final class WeightedRoundRobin extends HTTPBalance {

    private int index = 0;
    private final TreeRangeMap<Integer, Backend> backendsMap = TreeRangeMap.create();
    private int totalWeight = 0;

    public WeightedRoundRobin() {
        super(new NOOPSessionPersistence());
    }

    public WeightedRoundRobin(List<Backend> backends) {
        this(new NOOPSessionPersistence(), backends);
    }

    public WeightedRoundRobin(SessionPersistence<HTTPResponse, HTTPResponse, HTTPRequest, Backend> sessionPersistence, List<Backend> backends) {
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
    public HTTPResponse getResponse(HTTPRequest httpRequest) {
        HTTPResponse httpResponse = sessionPersistence.getBackend(httpRequest);
        if (httpResponse != null) {
            return httpResponse;
        }

        if (index >= totalWeight) {
            index = 0;
        }

        Backend backend = backendsMap.get(index);
        index++;
        return sessionPersistence.addRoute(httpRequest, backend);
    }
}
