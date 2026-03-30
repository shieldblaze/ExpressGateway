/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.servicediscovery.server;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Request body for bulk register/deregister operations.
 * Allows clients to register or deregister multiple nodes in a single HTTP call,
 * reducing network round-trips during fleet-wide deployments.
 */
public final class BulkRequest {

    @JsonProperty("Nodes")
    private List<Node> nodes;

    @JsonProperty("TTLSeconds")
    private long ttlSeconds;

    public BulkRequest() {
    }

    public BulkRequest(List<Node> nodes, long ttlSeconds) {
        this.nodes = nodes;
        this.ttlSeconds = ttlSeconds;
    }

    public List<Node> nodes() {
        return nodes;
    }

    public long ttlSeconds() {
        return ttlSeconds;
    }

    /**
     * Validate all nodes in the bulk request.
     *
     * @throws IllegalArgumentException if any node is invalid
     * @throws NullPointerException     if nodes list is null
     */
    public void validate() {
        if (nodes == null || nodes.isEmpty()) {
            throw new IllegalArgumentException("Nodes list must not be null or empty");
        }
        for (Node node : nodes) {
            node.validate();
        }
        if (ttlSeconds < 0) {
            throw new IllegalArgumentException("TTLSeconds must be >= 0, got: " + ttlSeconds);
        }
    }
}
