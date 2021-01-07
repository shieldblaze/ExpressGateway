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
package com.shieldblaze.expressgateway.protocol.http.loadbalancer;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.protocol.http.HTTPServerInitializer;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for {@link HTTPLoadBalancer}
 */
public final class HTTPLoadBalancerBuilder {
    private InetSocketAddress bindAddress;
    private CoreConfiguration coreConfiguration;
    private HTTPConfiguration httpConfiguration;
    private Cluster cluster;
    private L4FrontListener l4FrontListener;
    private HTTPServerInitializer httpServerInitializer;
    private TLSServerConfiguration tlsServer;
    private TLSClientConfiguration tlsClient;

    private HTTPLoadBalancerBuilder() {
        // Prevent outside initialization
    }

    public static HTTPLoadBalancerBuilder newBuilder() {
        return new HTTPLoadBalancerBuilder();
    }

    public HTTPLoadBalancerBuilder withCoreConfiguration(CoreConfiguration coreConfiguration) {
        this.coreConfiguration = coreConfiguration;
        return this;
    }

    public HTTPLoadBalancerBuilder withHTTPConfiguration(HTTPConfiguration httpConfiguration) {
        this.httpConfiguration = httpConfiguration;
        return this;
    }

    public HTTPLoadBalancerBuilder withCluster(Cluster cluster) {
        this.cluster = cluster;
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

    /**
     * Set {@link TLSClientConfiguration} for Client
     */
    public HTTPLoadBalancerBuilder withTLSForClient(TLSClientConfiguration tlsClient) {
        this.tlsClient = tlsClient;
        return this;
    }

    /**
     * Set {@link TLSServerConfiguration} for Server
     */
    public HTTPLoadBalancerBuilder withTLSForServer(TLSServerConfiguration tlsServer) {
        this.tlsServer = tlsServer;
        return this;
    }

    public HTTPLoadBalancer build() {
        Objects.requireNonNull(bindAddress, "BindAddress");
        Objects.requireNonNull(l4FrontListener, "L4FrontListener");
        Objects.requireNonNull(httpServerInitializer, "HTTPServerInitializer");
        Objects.requireNonNull(cluster, "Cluster");
        Objects.requireNonNull(coreConfiguration, "CoreConfiguration");
        Objects.requireNonNull(httpConfiguration, "HTTPConfiguration");

        return new HTTPLoadBalancer(
                bindAddress,
                l4FrontListener,
                cluster,
                coreConfiguration,
                tlsServer,
                tlsClient,
                httpConfiguration,
                httpServerInitializer
        );
    }
}
