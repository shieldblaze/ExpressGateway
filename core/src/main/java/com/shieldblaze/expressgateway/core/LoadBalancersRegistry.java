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
package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;

import java.util.Collections;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * Global {@link LoadBalancersRegistry} Registry
 * to keep track of all {@link L4LoadBalancer} running.
 */
public final class LoadBalancersRegistry {

    private static final List<L4LoadBalancer> loadBalancers = new CopyOnWriteArrayList<>();

    public static boolean addLoadBalancer(L4LoadBalancer l4LoadBalancer) {
        if (loadBalancers.contains(l4LoadBalancer)) {
            return false;
        }

        return loadBalancers.add(l4LoadBalancer);
    }

    public static boolean removeLoadBalancer(L4LoadBalancer l4LoadBalancer) {
        return loadBalancers.remove(l4LoadBalancer);
    }

    public static L4LoadBalancer id(String id) {
        for (L4LoadBalancer l4LoadBalancer : loadBalancers) {
            if (l4LoadBalancer.ID.equalsIgnoreCase(id)) {
                return l4LoadBalancer;
            }
        }

        return null;
    }

    public static List<L4LoadBalancer> getAll() {
        return Collections.unmodifiableList(loadBalancers);
    }
}
