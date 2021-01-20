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

import com.shieldblaze.expressgateway.backend.Node;

/**
 * <p> Session Persistence is used to route a request to specific {@linkplain Node}. </p>
 *
 * @param <REQUEST>  Request Type for {@linkplain #node(Request)}
 * @param <RESPONSE> Response Type for {@linkplain #addRoute(KEY, VALUE)} and {@linkplain #removeRoute(KEY, VALUE)}
 * @param <KEY>      Key Type to use for {@linkplain #addRoute(KEY, VALUE)} and {@linkplain #removeRoute(KEY, VALUE)}
 * @param <VALUE>    Value Type to use for {@linkplain #addRoute(KEY, VALUE)}, {@linkplain #removeRoute(KEY, VALUE)} and {@linkplain #remove(VALUE)}
 */
public interface SessionPersistence<REQUEST, RESPONSE, KEY, VALUE> {

    /**
     * Get {@link Node}
     *
     * @return {@link Node} is route is available else {@code null}
     */
    REQUEST node(Request request);

    /**
     * Add a Key-Value which maps to {@linkplain VALUE}
     */
    RESPONSE addRoute(KEY key, VALUE value);

    /**
     * Remove a Key-Value which maps to {@linkplain VALUE}
     *
     * @return Returns {@code true} if removal was successful else {@code false}
     */
    boolean removeRoute(KEY key, VALUE value);

    /**
     * Remove a value
     *
     * @param value {@linkplain VALUE} to be removed
     * @return Returns {@code true} if removal was successful else {@code false}
     */
    boolean remove(VALUE value);

    /**
     * Clear all Key-Value entries
     */
    void clear();

    /**
     * Name of this Session Persistence
     */
    String name();
}
