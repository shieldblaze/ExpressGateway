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
import com.shieldblaze.expressgateway.backend.connection.ClusterConnectionPool;
import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.server.L7FrontListener;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;

import java.net.InetSocketAddress;

/**
 * Default implementation of {@link HTTPLoadBalancer}
 */
final class DefaultHTTPLoadBalancer extends HTTPLoadBalancer {

    /**
     * @param bindAddress         {@link InetSocketAddress} on which {@link L7FrontListener} will bind and listen.
     * @param HTTPBalance         {@link HTTPBalance} for Load Balance
     * @param l7FrontListener     {@link L7FrontListener} for listening and handling traffic
     * @param cluster             {@link Cluster} to be Load Balanced
     * @param commonConfiguration {@link CommonConfiguration} to be applied
     * @param clusterConnectionPool   {@link ClusterConnectionPool} to use
     * @param httpConfiguration   {@link HTTPConfiguration} to be applied
     * @throws NullPointerException If any parameter is {@code null}
     */
    DefaultHTTPLoadBalancer(InetSocketAddress bindAddress, HTTPBalance HTTPBalance, L7FrontListener l7FrontListener, Cluster cluster,
                            CommonConfiguration commonConfiguration, ClusterConnectionPool clusterConnectionPool, HTTPConfiguration httpConfiguration,
                            TLSConfiguration tlsClient, TLSConfiguration tlsServer) {
        super(bindAddress, HTTPBalance, l7FrontListener, cluster, commonConfiguration, clusterConnectionPool, httpConfiguration, tlsClient, tlsServer);
    }
}
