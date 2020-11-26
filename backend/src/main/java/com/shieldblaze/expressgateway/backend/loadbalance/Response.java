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
package com.shieldblaze.expressgateway.backend.loadbalance;

import com.shieldblaze.expressgateway.backend.Node;

/**
 * {@linkplain Response} contains selected {@linkplain Node}
 */
public abstract class Response {
    private final Node node;

    /**
     * Create a new {@link Response} Instance
     *
     * @param node Selected {@linkplain Node} for the request
     */
    public Response(Node node) {
        this.node = node;
    }

    /**
     * Get selected {@linkplain Node}
     */
    public Node node() {
        return node;
    }

    @Override
    public String toString() {
        return "Response{" +
                "backend=" + node +
                '}';
    }
}
