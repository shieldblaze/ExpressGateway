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

public class LoadBalancerProperties {
    private final L4LoadBalancer l4LoadBalancer;
    private final L4FrontListenerStartupEvent startupEvent;
    private final long startMillis = System.currentTimeMillis();

    public LoadBalancerProperties(L4LoadBalancer l4LoadBalancer, L4FrontListenerStartupEvent startupEvent) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.startupEvent = startupEvent;
    }

    public L4LoadBalancer l4LoadBalancer() {
        return l4LoadBalancer;
    }

    public L4FrontListenerStartupEvent startupEvent() {
        return startupEvent;
    }

    public long startMillis() {
        return startMillis;
    }
}
