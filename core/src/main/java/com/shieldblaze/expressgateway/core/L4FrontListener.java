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
package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;

public abstract class L4FrontListener {

    private L4LoadBalancer l4LoadBalancer;

    public abstract L4FrontListenerStartupEvent start();

    public abstract L4FrontListenerStopEvent stop();

    /**
     * Returns {@link L4LoadBalancer} associated with this listener
     */
    protected L4LoadBalancer l4LoadBalancer() {
        return l4LoadBalancer;
    }

    /**
     * This method is automatically called by {@link L4LoadBalancer} while initializing.
     *
     * @param l4LoadBalancer {@link L4LoadBalancer} Instance
     * @throws UnsupportedOperationException If {@link L4LoadBalancer} is tried to be set again
     */
    public L4FrontListener l4LoadBalancer(L4LoadBalancer l4LoadBalancer) {
        if (this.l4LoadBalancer != null) {
            throw new UnsupportedOperationException("L4LoadBalancer is already set");
        }
        this.l4LoadBalancer = l4LoadBalancer;
        return this;
    }
}
