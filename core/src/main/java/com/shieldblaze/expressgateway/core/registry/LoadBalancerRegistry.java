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

import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public final class LoadBalancerRegistry {

    public static final Map<String, LoadBalancerProperties> REGISTRY = new ConcurrentHashMap<>();

    public static LoadBalancerProperties get(String loadBalancerID) {
        return REGISTRY.get(loadBalancerID);
    }

    public static void add(L4LoadBalancer l4LoadBalancer, LoadBalancerProperties loadBalancerProperties) {
        REGISTRY.put(l4LoadBalancer.ID, loadBalancerProperties);
    }

    public static LoadBalancerProperties remove(String loadBalancerID) {
        return REGISTRY.remove(loadBalancerID);
    }

    private LoadBalancerRegistry() {
        // Prevent outside initialization
    }
}
