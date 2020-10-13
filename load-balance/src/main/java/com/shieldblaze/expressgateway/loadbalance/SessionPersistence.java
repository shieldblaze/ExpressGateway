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
import com.shieldblaze.expressgateway.loadbalance.Request;

import java.net.InetSocketAddress;

/**
 * Session Persistence is used to route a request to specific {@linkplain Backend}.
 *
 * @param <R> Response for {@linkplain #getBackend(Request)}
 * @param <K> Key to use for {@linkplain #addRoute(Object, Object)}
 * @param <V> Value to use for {@linkplain #addRoute(Object, Object)}
 */
public interface SessionPersistence<R, K, V> {

    /**
     * Get {@link Backend}
     *
     * @return {@link Backend} is route is available else {@code null}
     */
    R getBackend(Request request);

    /**
     * Add a Key-Value which maps to {@linkplain V}
     */
    void addRoute(K key, V value);
}
