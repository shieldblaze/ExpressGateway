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
package com.shieldblaze.expressgateway.core.cluster;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.common.utils.MathUtil;

import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;

/**
 * This class holds all information about all load balancers.
 */
public final class CoreContext {

    /**
     * Mapping of Load Balancer ID with {@link LoadBalancerContext}
     */
    private static final Map<String, LoadBalancerContext> REGISTRY = new ConcurrentHashMap<>();

    /**
     * Get mapped {@link LoadBalancerContext} using Load Balancer ID
     *
     * @param id Load Balancer ID
     * @return {@link LoadBalancerContext} Instance
     * @throws NullPointerException If {@link LoadBalancerContext} is not found with the ID
     */
    public static LoadBalancerContext get(String id) {
        Objects.requireNonNull(id, "ID cannot be 'null'");

        LoadBalancerContext property = REGISTRY.get(id);
        Objects.requireNonNull(property, "Load Balancer was not found with the ID: " + id);

        return property;
    }

    /**
     * Add mapping to {@link LoadBalancerContext} using Load Balancer ID
     *
     * @param id       Load Balancer ID
     * @param context {@link LoadBalancerContext} Instance
     * @throws NullPointerException If {@code id} or {@link LoadBalancerContext} is 'null'
     */
    public static void add(String id, LoadBalancerContext context) {
        Objects.requireNonNull(id, "ID cannot be 'null'");
        Objects.requireNonNull(context, "Property cannot be 'null'");

        REGISTRY.put(id, context);
    }

    /**
     * Remove mapping of {@link LoadBalancerContext} using Load Balancer ID
     *
     * @param id Load Balancer ID
     * @return {@link LoadBalancerContext} Instance is successfully removed else {@code null}
     */
    public static LoadBalancerContext remove(String id) {
        Objects.requireNonNull(id, "ID cannot be 'null'");
        return REGISTRY.remove(id);
    }

    /**
     * Get total connections across all load balancers.
     */
    public int totalActiveConnections() {
        return REGISTRY.values()
                .stream()
                .mapToInt(loadBalancerProperty -> loadBalancerProperty.l4LoadBalancer()
                        .connectionTracker()
                        .connections())
                .sum();
    }

    /**
     * Get total connections load across all load balancers
     */
    public long totalConnections() {
         return REGISTRY.values()
                .stream()
                .mapToLong(value -> value.l4LoadBalancer()
                        .clusters()
                        .values()
                        .stream()
                        .mapToLong(cluster -> cluster.nodes()
                                .stream()
                                .mapToInt(Node::maxConnections)
                                .sum())
                        .sum())
                .sum();
    }

    private CoreContext() {
        // Prevent outside initialization
    }
}
