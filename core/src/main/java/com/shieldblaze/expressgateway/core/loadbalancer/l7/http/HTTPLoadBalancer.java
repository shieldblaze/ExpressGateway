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
package com.shieldblaze.expressgateway.core.loadbalancer.l7.http;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.connection.ConnectionManager;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.L7LoadBalancer;
import com.shieldblaze.expressgateway.core.server.L7FrontListener;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * HTTP Load Balancer
 */
public abstract class HTTPLoadBalancer extends L7LoadBalancer {

    private final HTTPConfiguration httpConfiguration;

    /**
     * @param bindAddress         {@link InetSocketAddress} on which {@link L7FrontListener} will bind and listen.
     * @param HTTPBalance         {@link HTTPBalance} for Load Balance
     * @param l7FrontListener     {@link L7FrontListener} for listening and handling traffic
     * @param cluster             {@link Cluster} to be Load Balanced
     * @param commonConfiguration {@link CommonConfiguration} to be applied
     * @param connectionManager   {@link ConnectionManager} to use
     * @throws NullPointerException If any parameter is {@code null}
     */
    public HTTPLoadBalancer(InetSocketAddress bindAddress, HTTPBalance HTTPBalance, L7FrontListener l7FrontListener, Cluster cluster,
                            CommonConfiguration commonConfiguration, ConnectionManager connectionManager, HTTPConfiguration httpConfiguration) {
        super(bindAddress, HTTPBalance, l7FrontListener, cluster, commonConfiguration, connectionManager);
        this.httpConfiguration = Objects.requireNonNull(httpConfiguration, "httpConfiguration");
    }

    /**
     * Get {@link HTTPConfiguration} Instance for this {@link HTTPLoadBalancer}
     */
    public HTTPConfiguration getHTTPConfiguration() {
        return httpConfiguration;
    }
}
