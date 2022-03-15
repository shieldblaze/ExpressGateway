/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.protocol.http.DefaultHTTPServerInitializer;
import com.shieldblaze.expressgateway.protocol.http.HTTPServerInitializer;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for {@link HTTPLoadBalancer}
 */
public final class HTTPLoadBalancerBuilder {
    private String name;
    private InetSocketAddress bindAddress;
    private ConfigurationContext configurationContext = ConfigurationContext.DEFAULT;
    private L4FrontListener l4FrontListener;
    private HTTPServerInitializer httpServerInitializer = new DefaultHTTPServerInitializer();

    private HTTPLoadBalancerBuilder() {
        // Prevent outside initialization
    }

    public static HTTPLoadBalancerBuilder newBuilder() {
        return new HTTPLoadBalancerBuilder();
    }

    public HTTPLoadBalancerBuilder withName(String name) {
        this.name = name;
        return this;
    }

    public HTTPLoadBalancerBuilder withConfigurationContext(ConfigurationContext configurationContext) {
        this.configurationContext = configurationContext;
        return this;
    }

    public HTTPLoadBalancerBuilder withHTTPInitializer(HTTPServerInitializer httpServerInitializer) {
        this.httpServerInitializer = httpServerInitializer;
        return this;
    }

    public HTTPLoadBalancerBuilder withL4FrontListener(L4FrontListener l4FrontListener) {
        this.l4FrontListener = l4FrontListener;
        return this;
    }

    /**
     * Set {@link InetSocketAddress} where {@link HTTPLoadBalancer} will bind and listen
     */
    public HTTPLoadBalancerBuilder withBindAddress(InetSocketAddress bindAddress) {
        this.bindAddress = bindAddress;
        return this;
    }

    public HTTPLoadBalancer build() {
        Objects.requireNonNull(bindAddress, "BindAddress");
        Objects.requireNonNull(l4FrontListener, "L4FrontListener");

        return new HTTPLoadBalancer(
                name,
                bindAddress,
                l4FrontListener,
                configurationContext,
                httpServerInitializer
        );
    }
}
