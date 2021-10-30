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

import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;

import java.util.Objects;

/**
 * This class holds basic context of a load balancer
 * such as startup timestamp, {@link L4FrontListenerStartupEvent} instance, etc.
 */
public final class LoadBalancerContext {

    // Load Balancer start time
    public static final long STARTUP_TIMESTAMP = System.currentTimeMillis();
    private final L4LoadBalancer l4LoadBalancer;

    /**
     * {@link L4LoadBalancer}'s {@link L4FrontListenerStartupEvent} Instance
     */
    private L4FrontListenerStartupEvent startupEvent;

    /**
     * Create a new {@link LoadBalancerContext} instance
     *
     * @param l4LoadBalancer {@link L4LoadBalancer} instance
     * @param startupEvent   {@link L4FrontListenerStartupEvent} instance
     * @throws NullPointerException If any parameter is 'null'
     */
    public LoadBalancerContext(L4LoadBalancer l4LoadBalancer, L4FrontListenerStartupEvent startupEvent) {
        this.l4LoadBalancer = Objects.requireNonNull(l4LoadBalancer, "L4LoadBalancer cannot be 'null'");
        modifyStartupEvent(startupEvent);
    }

    /**
     * Return the associated {@link L4LoadBalancer} Instance
     */
    public L4LoadBalancer l4LoadBalancer() {
        return l4LoadBalancer;
    }

    /**
     * Modify the {@link L4FrontListenerStartupEvent} instance with a new instance
     * @param startupEvent {@link L4FrontListenerStartupEvent} instance
     * @throws NullPointerException If the parameter is `null`.
     */
    public void modifyStartupEvent(L4FrontListenerStartupEvent startupEvent) {
        this.startupEvent = Objects.requireNonNull(startupEvent, "L4FrontListenerStartupEvent cannot be 'null'");
    }

    /**
     * Return the associated {@link L4FrontListenerStartupEvent} Instance
     */
    public L4FrontListenerStartupEvent modifyStartupEvent() {
        return startupEvent;
    }
}
