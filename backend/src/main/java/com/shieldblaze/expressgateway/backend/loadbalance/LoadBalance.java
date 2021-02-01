/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend.loadbalance;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

import java.io.Closeable;

/**
 * Base Implementation for Load Balance
 */
@SuppressWarnings("rawtypes")
public abstract class LoadBalance<REQUEST, RESPONSE, KEY, VALUE> implements EventListener, Closeable {

    protected final SessionPersistence<REQUEST, RESPONSE, KEY, VALUE> sessionPersistence;
    protected Cluster cluster;

    /**
     * Create {@link LoadBalance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    @NonNull
    public LoadBalance(SessionPersistence<REQUEST, RESPONSE, KEY, VALUE> sessionPersistence) {
        this.sessionPersistence = sessionPersistence;
    }

    /**
     * @param cluster {@link Cluster} to be load balanced
     */
    @NonNull
    public void cluster(Cluster cluster) {
        this.cluster = cluster;
    }

    /**
     * Generate a Load-Balance {@linkplain Response} for {@linkplain Request}
     *
     * @param request {@linkplain Request}
     * @return {@linkplain Response} if successful
     * @throws LoadBalanceException In case of some error while generating {@linkplain Response}
     */
    @NonNull
    public abstract Response response(Request request) throws LoadBalanceException;

    /**
     * Name of this Load Balance
     */
    public abstract String name();

    public SessionPersistence<REQUEST, RESPONSE, KEY, VALUE> sessionPersistence() {
        return sessionPersistence;
    }
}
