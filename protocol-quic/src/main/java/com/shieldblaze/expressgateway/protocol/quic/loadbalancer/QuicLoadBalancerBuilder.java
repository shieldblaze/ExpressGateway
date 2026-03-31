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
package com.shieldblaze.expressgateway.protocol.quic.loadbalancer;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.protocol.udp.UDPListener;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for {@link QuicLoadBalancer}.
 *
 * <p>Creates an L4 QUIC load balancer that transparently forwards QUIC datagrams
 * without terminating the QUIC connection. Uses {@link UDPListener} as the frontend
 * since it operates at the raw datagram level.</p>
 */
public final class QuicLoadBalancerBuilder {

    private String name;
    private InetSocketAddress bindAddress;
    private ConfigurationContext configurationContext = ConfigurationContext.DEFAULT;
    private L4FrontListener l4FrontListener;

    private QuicLoadBalancerBuilder() {
        // Prevent outside initialization
    }

    public static QuicLoadBalancerBuilder newBuilder() {
        return new QuicLoadBalancerBuilder();
    }

    public QuicLoadBalancerBuilder withName(String name) {
        this.name = name;
        return this;
    }

    public QuicLoadBalancerBuilder withBindAddress(InetSocketAddress bindAddress) {
        this.bindAddress = bindAddress;
        return this;
    }

    public QuicLoadBalancerBuilder withConfigurationContext(ConfigurationContext configurationContext) {
        this.configurationContext = configurationContext;
        return this;
    }

    /**
     * Set a custom frontend listener. Defaults to {@link UDPListener} if not specified.
     */
    public QuicLoadBalancerBuilder withL4FrontListener(L4FrontListener l4FrontListener) {
        this.l4FrontListener = l4FrontListener;
        return this;
    }

    /**
     * Build the {@link QuicLoadBalancer}.
     *
     * @throws NullPointerException  if bindAddress is null
     * @throws IllegalStateException if QUIC is not enabled in the configuration
     */
    public QuicLoadBalancer build() {
        Objects.requireNonNull(bindAddress, "BindAddress");

        if (!configurationContext.quicConfiguration().enabled()) {
            throw new IllegalStateException(
                    "QUIC is not enabled in QuicConfiguration. Set enabled=true before building.");
        }

        if (l4FrontListener == null) {
            l4FrontListener = new UDPListener();
        }

        return new QuicLoadBalancer(
                name,
                bindAddress,
                l4FrontListener,
                configurationContext
        );
    }
}
