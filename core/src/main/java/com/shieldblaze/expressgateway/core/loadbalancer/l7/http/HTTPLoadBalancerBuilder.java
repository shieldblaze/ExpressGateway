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

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.server.L7FrontListener;
import com.shieldblaze.expressgateway.core.server.http.HTTPListener;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for {@link HTTPLoadBalancer}
 */
public final class HTTPLoadBalancerBuilder {
    private InetSocketAddress bindAddress;
    private CommonConfiguration commonConfiguration;
    private HTTPConfiguration httpConfiguration;
    private L7Balance l7Balance;
    private Cluster cluster;
    private HTTPListener httpListener;

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

    public HTTPLoadBalancerBuilder withL7Balance(L7Balance l7Balance) {
        this.l7Balance = l7Balance;
        return this;
    }

    public HTTPLoadBalancerBuilder withCluster(Cluster cluster) {
        this.cluster = cluster;
        return this;
    }

    public HTTPLoadBalancerBuilder withL7FrontListener(HTTPListener httpListener) {
        this.httpListener = httpListener;
        return this;
    }

    /**
     * Set {@link InetSocketAddress} where {@link L7FrontListener} will bind and listen
     */
    public HTTPLoadBalancerBuilder withBindAddress(InetSocketAddress bindAddress) {
        this.bindAddress = bindAddress;
        return this;
    }

    public HTTPLoadBalancer build() {
        HTTPLoadBalancer HTTPLoadBalancer = new HTTPLoadBalancer(
                Objects.requireNonNull(bindAddress, "bindAddress"),
                Objects.requireNonNull(l7Balance, "l7Balance"),
                Objects.requireNonNull(httpListener, "httpListener"),
                Objects.requireNonNull(cluster, "cluster"),
                Objects.requireNonNull(commonConfiguration, "commonConfiguration"),
                Objects.requireNonNull(httpConfiguration, "httpConfiguration")
        );
        l7Balance.setBackends(cluster.getBackends());
        return HTTPLoadBalancer;
    }
}
