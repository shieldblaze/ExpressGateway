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

import com.shieldblaze.expressgateway.core.concurrent.async.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.L7LoadBalancer;
import com.shieldblaze.expressgateway.core.server.L7FrontListener;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.concurrent.CompletableFuture;

/**
 * HTTP Load Balancer
 */
public final class HTTPLoadBalancer extends L7LoadBalancer {

    private final HTTPConfiguration httpConfiguration;

    /**
     * @see L7FrontListener
     */
    HTTPLoadBalancer(InetSocketAddress bindAddress, L7Balance l7Balance, L7FrontListener l7FrontListener, Cluster cluster,
                            CommonConfiguration commonConfiguration, HTTPConfiguration httpConfiguration) {
        super(bindAddress, l7Balance, l7FrontListener, cluster, commonConfiguration);
        this.httpConfiguration = httpConfiguration;
    }

    public HTTPConfiguration getHTTPConfiguration() {
        return httpConfiguration;
    }
}
