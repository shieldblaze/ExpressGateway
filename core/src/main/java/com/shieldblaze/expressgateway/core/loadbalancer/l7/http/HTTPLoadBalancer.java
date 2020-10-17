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

import com.shieldblaze.expressgateway.backend.Cluster;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.L7LoadBalancer;
import com.shieldblaze.expressgateway.core.server.http.HTTPFrontListener;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * HTTP Load Balancer
 */
public abstract class HTTPLoadBalancer extends L7LoadBalancer {

    private final HTTPConfiguration httpConfiguration;

    /**
     * @throws NullPointerException If any parameter is {@code null}
     * @see L7LoadBalancer
     */
    HTTPLoadBalancer(InetSocketAddress bindAddress, HTTPBalance HTTPBalance, HTTPFrontListener httpFrontListener, Cluster cluster,
                     CommonConfiguration commonConfiguration, HTTPConfiguration httpConfiguration) {
        super(bindAddress, HTTPBalance, httpFrontListener, cluster, commonConfiguration);
        this.httpConfiguration = Objects.requireNonNull(httpConfiguration, "httpConfiguration");
    }

    /**
     * Get {@link HTTPConfiguration} Instance for this {@link HTTPLoadBalancer}
     */
    public HTTPConfiguration getHTTPConfiguration() {
        return httpConfiguration;
    }
}
