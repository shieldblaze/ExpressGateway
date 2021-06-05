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
package com.shieldblaze.expressgateway.protocol.http.loadbalancer;

import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.HTTPServerInitializer;

import java.net.InetSocketAddress;

/**
 * HTTP Load Balancer
 */
public class HTTPLoadBalancer extends L4LoadBalancer {

    private final HTTPConfiguration httpConfiguration;

    HTTPLoadBalancer(String name, InetSocketAddress bindAddress, L4FrontListener l4FrontListener,
                     CoreConfiguration coreConfiguration, TLSConfiguration tlsForServer, TLSConfiguration tlsForClient,
                     HTTPConfiguration httpConfiguration, HTTPServerInitializer httpServerInitializer) {
        super(name, bindAddress, l4FrontListener, coreConfiguration, tlsForServer, tlsForClient, httpServerInitializer);
        this.httpConfiguration = httpConfiguration;
        httpServerInitializer.httpLoadBalancer(this);
    }

    /**
     * Get {@link HTTPConfiguration} Instance for this {@link HTTPLoadBalancer}
     */
    public HTTPConfiguration httpConfiguration() {
        return httpConfiguration;
    }

    @Override
    public String type() {
        return "L7/HTTP";
    }
}
