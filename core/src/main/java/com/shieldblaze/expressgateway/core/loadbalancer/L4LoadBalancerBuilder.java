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
package com.shieldblaze.expressgateway.core.loadbalancer;

import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import io.netty.channel.ChannelHandler;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for {@link L4LoadBalancer}
 */
public final class L4LoadBalancerBuilder {

    private String name;
    private InetSocketAddress bindAddress;
    private L4FrontListener l4FrontListener;
    private CoreConfiguration coreConfiguration = CoreConfiguration.INSTANCE;
    private TLSConfiguration tlsForServer;
    private TLSConfiguration tlsForClient;
    private ChannelHandler channelHandler;

    private L4LoadBalancerBuilder() {
        // Prevent outside initialization
    }

    public static L4LoadBalancerBuilder newBuilder() {
        return new L4LoadBalancerBuilder();
    }

    public L4LoadBalancerBuilder withName(String name) {
        this.name = name;
        return this;
    }

    public L4LoadBalancerBuilder withBindAddress(InetSocketAddress bindAddress) {
        this.bindAddress = bindAddress;
        return this;
    }

    public L4LoadBalancerBuilder withL4FrontListener(L4FrontListener l4FrontListener) {
        this.l4FrontListener = l4FrontListener;
        return this;
    }

    public L4LoadBalancerBuilder withCoreConfiguration(CoreConfiguration coreConfiguration) {
        this.coreConfiguration = coreConfiguration;
        return this;
    }

    public L4LoadBalancerBuilder withTLSForServer(TLSConfiguration tlsForServer) {
        this.tlsForServer = tlsForServer;
        return this;
    }

    public L4LoadBalancerBuilder withTLSForClient(TLSConfiguration tlsForClient) {
        this.tlsForClient = tlsForClient;
        return this;
    }

    public L4LoadBalancerBuilder withChannelHandler(ChannelHandler channelHandler) {
        this.channelHandler = channelHandler;
        return this;
    }

    public L4LoadBalancer build() {
        Objects.requireNonNull(bindAddress, "Bind Address");
        Objects.requireNonNull(l4FrontListener, "L4 FrontListener");
        Objects.requireNonNull(coreConfiguration, "Core Configuration");

        return new DefaultL4LoadBalancer(name, bindAddress, l4FrontListener, coreConfiguration, tlsForServer, tlsForClient, channelHandler);
    }
}
