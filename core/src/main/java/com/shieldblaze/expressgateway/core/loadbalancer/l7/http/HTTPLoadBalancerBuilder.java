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
import com.shieldblaze.expressgateway.core.server.http.HTTPFrontListener;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for {@link HTTPLoadBalancer}
 */
public final class HTTPLoadBalancerBuilder {
    private InetSocketAddress bindAddress;
    private CommonConfiguration commonConfiguration;
    private HTTPConfiguration httpConfiguration;
    private HTTPBalance httpBalance;
    private Cluster cluster;
    private HTTPFrontListener httpFrontListener;
    private HTTPLoadBalancer httpLoadBalancer;
    private ClusterConnectionPool clusterConnectionPool;
    private TLSConfiguration tlsServer;
    private TLSConfiguration tlsClient;

    private HTTPLoadBalancerBuilder() {
        // Prevent outside initialization
    }

    public static HTTPLoadBalancerBuilder newBuilder() {
        return new HTTPLoadBalancerBuilder();
    }

    public HTTPLoadBalancerBuilder withCommonConfiguration(CommonConfiguration commonConfiguration) {
        this.commonConfiguration = commonConfiguration;
        return this;
    }

    public HTTPLoadBalancerBuilder withHTTPConfiguration(HTTPConfiguration httpConfiguration) {
        this.httpConfiguration = httpConfiguration;
        return this;
    }

    public HTTPLoadBalancerBuilder withL7Balance(HTTPBalance HTTPBalance) {
        this.httpBalance = HTTPBalance;
        return this;
    }

    public HTTPLoadBalancerBuilder withCluster(Cluster cluster) {
        this.cluster = cluster;
        return this;
    }

    public HTTPLoadBalancerBuilder withHTTPFrontListener(HTTPFrontListener httpFrontListener) {
        this.httpFrontListener = httpFrontListener;
        return this;
    }

    /**
     * Set {@link InetSocketAddress} where {@link L7FrontListener} will bind and listen
     */
    public HTTPLoadBalancerBuilder withBindAddress(InetSocketAddress bindAddress) {
        this.bindAddress = bindAddress;
        return this;
    }

    /**
     * Set {@link HTTPLoadBalancer} to use
     */
    public HTTPLoadBalancerBuilder withHTTPLoadBalancer(HTTPLoadBalancer httpLoadBalancer) {
        this.httpLoadBalancer = httpLoadBalancer;
        return this;
    }

    /**
     * Set {@link ClusterConnectionPool} to use
     */
    public HTTPLoadBalancerBuilder withClusterConnectionPool(ClusterConnectionPool clusterConnectionPool) {
        this.clusterConnectionPool = clusterConnectionPool;
        return this;
    }

    /**
     * Set {@link TLSConfiguration} for Client
     */
    public HTTPLoadBalancerBuilder withTLSForClient(TLSConfiguration tlsClient) {
        this.tlsClient = tlsClient;
        return this;
    }

    /**
     * Set {@link TLSConfiguration} for Server
     */
    public HTTPLoadBalancerBuilder withTLSForServer(TLSConfiguration tlsServer) {
        this.tlsServer = tlsServer;
        return this;
    }

    public HTTPLoadBalancer build() {
        if (httpLoadBalancer == null) {
            httpLoadBalancer = new DefaultHTTPLoadBalancer(
                    Objects.requireNonNull(bindAddress, "bindAddress"),
                    Objects.requireNonNull(httpBalance, "httpBalance"),
                    Objects.requireNonNull(httpFrontListener, "httpFrontListener"),
                    Objects.requireNonNull(cluster, "cluster"),
                    Objects.requireNonNull(commonConfiguration, "commonConfiguration"),
                    Objects.requireNonNull(clusterConnectionPool, "ClusterConnectionPool"),
                    Objects.requireNonNull(httpConfiguration, "httpConfiguration"),
                    tlsClient,
                    tlsServer
            );
        }
        httpBalance.setCluster(cluster);
        httpFrontListener.setL7LoadBalancer(httpLoadBalancer);
        return httpLoadBalancer;
    }
}
