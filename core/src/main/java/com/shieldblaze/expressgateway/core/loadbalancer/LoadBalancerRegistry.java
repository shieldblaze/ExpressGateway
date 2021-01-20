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
package com.shieldblaze.expressgateway.core.loadbalancer;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public class LoadBalancerRegistry {

    /**
     * Use {@link #add(L4LoadBalancer, LoadBalancerProperty)} to register {@link L4LoadBalancer}
     */
    public static final Map<L4LoadBalancer, LoadBalancerProperty> registry = new ConcurrentHashMap<>();

    public static void add(L4LoadBalancer l4LoadBalancer, LoadBalancerProperty loadBalancerProperty) {
        if (registry.containsKey(l4LoadBalancer)) {
            throw new IllegalArgumentException("LoadBalancer is already registered");
        }

        registry.put(l4LoadBalancer, loadBalancerProperty);
    }

    public static void remove(L4LoadBalancer l4LoadBalancer) {
        registry.remove(l4LoadBalancer);
    }
}
