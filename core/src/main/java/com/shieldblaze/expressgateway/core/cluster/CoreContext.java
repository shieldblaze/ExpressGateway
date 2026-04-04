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
import com.shieldblaze.expressgateway.core.exceptions.NotFoundException;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import lombok.extern.log4j.Log4j2;

import java.util.Collections;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

import static com.shieldblaze.expressgateway.common.utils.ObjectUtils.nonNullObject;

/**
 * {@link CoreContext} holds all the {@link L4LoadBalancer} instances.
 *
 * <p>This is a global static registry -- all load balancer instances are registered
 * here on creation and looked up by ID for management operations (control plane,
 * REST API, metrics collection).</p>
 */
@Log4j2
public final class CoreContext {

    /**
     * Mapping of Load Balancer ID with {@link L4LoadBalancer}
     */
    private static final Map<String, L4LoadBalancer> REGISTRY = new ConcurrentHashMap<>();

    /**
     * Get mapped {@link L4LoadBalancer} using Load Balancer ID
     *
     * @param id Load Balancer ID
     * @return {@link L4LoadBalancer} Instance
     * @throws NotFoundException    If {@link L4LoadBalancer} is not found with the ID
     * @throws NullPointerException If {@code id} is {@code null}
     */
    public static L4LoadBalancer getContext(String id) {
        nonNullObject(id, "ID");

        L4LoadBalancer property = REGISTRY.get(id);

        if (property == null) {
            throw new NotFoundException("Load Balancer was not found with the ID: " + id);
        }

        return property;
    }

    /**
     * Add mapping to {@link L4LoadBalancer} using Load Balancer ID
     *
     * @param id      Load Balancer ID
     * @param context {@link L4LoadBalancer} Instance
     * @throws NullPointerException If {@code id} or {@link L4LoadBalancer} is 'null'
     */
    public static void add(String id, L4LoadBalancer context) {
        nonNullObject(id, "ID");
        nonNullObject(context, "LoadBalancerContext");

        // CM-05: Use putIfAbsent() to eliminate the TOCTOU race between containsKey()
        // and put(). With ConcurrentHashMap, two threads calling add() concurrently with
        // the same ID could both pass containsKey() and overwrite each other's entry.
        // putIfAbsent() is atomic and returns null only if the key was absent.
        L4LoadBalancer existing = REGISTRY.putIfAbsent(id, context);
        if (existing != null) {
            throw new IllegalArgumentException("Load Balancer already exists with the ID: " + id);
        }
    }

    /**
     * Remove mapping of {@link L4LoadBalancer} using Load Balancer ID
     *
     * @param id Load Balancer ID
     * @return {@link L4LoadBalancer} Instance is successfully removed else {@code null}
     */
    public static L4LoadBalancer remove(String id) {
        nonNullObject(id, "ID");
        return REGISTRY.remove(id);
    }

    /**
     * Get an unmodifiable view of all registered load balancers.
     */
    public static Map<String, L4LoadBalancer> all() {
        return Collections.unmodifiableMap(REGISTRY);
    }

    /**
     * Get the number of registered load balancers.
     */
    public static int size() {
        return REGISTRY.size();
    }

    /**
     * Get total active connections across all load balancers.
     */
    public static int totalActiveConnections() {
        int total = 0;
        for (L4LoadBalancer lb : REGISTRY.values()) {
            total += lb.connectionTracker().connections();
        }
        return total;
    }

    /**
     * Get the total maximum connections capacity across all load balancers.
     * This sums {@link Node#maxConnections()} for all online nodes across all clusters.
     */
    public static long totalConnectionCapacity() {
        long total = 0;
        for (L4LoadBalancer lb : REGISTRY.values()) {
            for (var cluster : lb.clusters().values()) {
                for (Node node : cluster.onlineNodes()) {
                    total += node.maxConnections();
                }
            }
        }
        return total;
    }

    private CoreContext() {
        // Prevent outside initialization
    }
}
