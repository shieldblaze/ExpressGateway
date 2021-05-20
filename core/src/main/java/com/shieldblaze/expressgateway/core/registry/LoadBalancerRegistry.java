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
package com.shieldblaze.expressgateway.core.registry;

import java.util.Collections;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;

/**
 * This class containing mapping operations of {@link LoadBalancerProperty}.
 */
public final class LoadBalancerRegistry {

    private LoadBalancerRegistry() {
        // Prevent outside initialization
    }

    /**
     * Mapping of Load Balancer ID with {@link LoadBalancerProperty}
     */
    private static final Map<String, LoadBalancerProperty> REGISTRY = new ConcurrentHashMap<>();

    /**
     * Get mapped {@link LoadBalancerProperty} using Load Balancer ID
     *
     * @param id Load Balancer ID
     * @return {@link LoadBalancerProperty} Instance
     * @throws NullPointerException If {@link LoadBalancerProperty} is not found with the ID
     */
    public static LoadBalancerProperty get(String id) {
        Objects.requireNonNull(id, "id");

        LoadBalancerProperty property = REGISTRY.get(id);
        if (property == null) {
            throw new NullPointerException("LoadBalancer not found with the ID: " + id);
        }

        return property;
    }

    /**
     * Add mapping to {@link LoadBalancerProperty} using Load Balancer ID
     *
     * @param id       Load Balancer ID
     * @param property {@link LoadBalancerProperty} Instance
     */
    public static void add(String id, LoadBalancerProperty property) {
        Objects.requireNonNull(id, "id");
        Objects.requireNonNull(property, "Property");
        REGISTRY.put(id, property);
    }

    /**
     * Remove mapping of {@link LoadBalancerProperty} using Load Balancer ID
     *
     * @param id Load Balancer ID
     * @return {@link LoadBalancerProperty} Instance is successfully removed else {@code null}
     */
    public static LoadBalancerProperty remove(String id) {
        Objects.requireNonNull(id, "id");
        return REGISTRY.remove(id);
    }

    /**
     * Get Registry Map
     */
    public static Map<String, LoadBalancerProperty> registry() {
        return Collections.unmodifiableMap(REGISTRY);
    }
}
