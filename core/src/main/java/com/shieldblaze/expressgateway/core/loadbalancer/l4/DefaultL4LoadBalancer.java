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
package com.shieldblaze.expressgateway.core.loadbalancer.l4;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Balance;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.concurrent.CompletableFuture;

/**
 * Default implementation for {@link L4LoadBalancer}
 */
final class DefaultL4LoadBalancer extends L4LoadBalancer {

    /**
     * @see L4LoadBalancer
     */
    DefaultL4LoadBalancer(InetSocketAddress bindAddress, L4Balance l4Balance, L4FrontListener l4FrontListener, Cluster cluster,
                          CommonConfiguration commonConfiguration) {
        super(bindAddress, l4Balance, l4FrontListener, cluster, commonConfiguration);
    }

    /**
     * {@inheritDoc}
     */
    @Override
    public List<CompletableFuture<L4FrontListenerEvent>> start() {
        return getL4FrontListener().start();
    }

    /**
     * {@inheritDoc}
     */
    @Override
    public CompletableFuture<Boolean> stop() {
        return getL4FrontListener().stop();
    }
}
