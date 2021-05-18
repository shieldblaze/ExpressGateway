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

import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;

public final class LoadBalancerProperty {

    /**
     * Load Balancer start time
     */
    private final long startMillis = System.currentTimeMillis();

    /**
     * {@link L4LoadBalancer} Instance
     */
    private final L4LoadBalancer l4LoadBalancer;

    /**
     * {@link L4LoadBalancer}'s {@link L4FrontListenerStartupEvent} Instance
     */
    private L4FrontListenerStartupEvent startupEvent;

    public LoadBalancerProperty(L4LoadBalancer l4LoadBalancer, L4FrontListenerStartupEvent startupEvent) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.startupEvent = startupEvent;
    }

    public long startMillis() {
        return startMillis;
    }

    public L4LoadBalancer l4LoadBalancer() {
        return l4LoadBalancer;
    }

    public void startupEvent(L4FrontListenerStartupEvent startupEvent) {
        this.startupEvent = startupEvent;
    }

    public L4FrontListenerStartupEvent startupEvent() {
        return startupEvent;
    }
}
