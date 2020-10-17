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
package com.shieldblaze.expressgateway.loadbalance;

import com.shieldblaze.expressgateway.backend.Backend;

import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/**
 * Base Implementation for Load Balance
 */
public abstract class LoadBalance<REQUEST, RESPONSE, KEY, VALUE> {
    protected final SessionPersistence<REQUEST, RESPONSE, KEY, VALUE> sessionPersistence;
    protected List<Backend> backends;

    /**
     * Create {@link LoadBalance} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Instance
     * @throws NullPointerException If {@link SessionPersistence} is {@code null}
     */
    public LoadBalance(SessionPersistence<REQUEST, RESPONSE, KEY, VALUE>  sessionPersistence) {
        this.sessionPersistence = Objects.requireNonNull(sessionPersistence, "sessionPersistence");
    }

    /**
     * Set Backends
     *
     * @param backends {@link List} of {@link Backend}
     * @throws IllegalArgumentException If {@link List} of {@link Backend} is Empty.
     * @throws NullPointerException     If {@link List} of {@link Backend} is {@code null}.
     */
    public void setBackends(List<Backend> backends) {
        Objects.requireNonNull(backends, "backends");
        if (backends.size() == 0) {
            throw new IllegalArgumentException("Backends List Cannot Be Empty");
        }
        this.backends = new ArrayList<>(backends);
    }

    public abstract Response getResponse(Request request);
}
