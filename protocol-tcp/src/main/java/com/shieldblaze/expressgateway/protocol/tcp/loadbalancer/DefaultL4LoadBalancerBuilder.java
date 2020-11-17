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
package com.shieldblaze.expressgateway.protocol.tcp.loadbalancer;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Balance;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.L4LoadBalancer;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for creating default {@link L4LoadBalancer}
 */
public final class DefaultL4LoadBalancerBuilder {
    private InetSocketAddress bindAddress;
    private CoreConfiguration coreConfiguration;
    private L4Balance l4Balance;
    private Cluster cluster;
    private L4FrontListener l4FrontListener;

    private DefaultL4LoadBalancerBuilder() {
        // Prevent outside initialization
    }

    /**
     * Create a new {@link DefaultL4LoadBalancerBuilder} Instance
     *
     * @return {@link DefaultL4LoadBalancerBuilder} Instance
     */
    public static DefaultL4LoadBalancerBuilder newBuilder() {
        return new DefaultL4LoadBalancerBuilder();
    }

    /**
     * Set {@link CoreConfiguration}
     */
    public DefaultL4LoadBalancerBuilder withCommonConfiguration(CoreConfiguration coreConfiguration) {
        this.coreConfiguration = coreConfiguration;
        return this;
    }

    /**
     * Set {@link L4Balance} Type
     */
    public DefaultL4LoadBalancerBuilder withL4Balance(L4Balance l4Balance) {
        this.l4Balance = l4Balance;
        return this;
    }

    /**
     * Set {@link Cluster} to Load Balance
     */
    public DefaultL4LoadBalancerBuilder withCluster(Cluster cluster) {
        this.cluster = cluster;
        return this;
    }

    /**
     * Set {@link L4FrontListener} to listen incoming traffic
     */
    public DefaultL4LoadBalancerBuilder withFrontListener(L4FrontListener l4FrontListener) {
        this.l4FrontListener = l4FrontListener;
        return this;
    }

    /**
     * Set {@link InetSocketAddress} where {@link L4FrontListener} will bind and listen
     */
    public DefaultL4LoadBalancerBuilder withBindAddress(InetSocketAddress bindAddress) {
        this.bindAddress = bindAddress;
        return this;
    }

    /**
     * Build {@link DefaultL4LoadBalancer} Instance
     *
     * @return {@link DefaultL4LoadBalancer} Instance
     * @throws NullPointerException If a required value if {@code null}
     */
    public DefaultL4LoadBalancer build() {
        DefaultL4LoadBalancer defaultL4LoadBalancer = new DefaultL4LoadBalancer(
                Objects.requireNonNull(bindAddress, "bindAddress"),
                Objects.requireNonNull(l4Balance, "L4Balance"),
                Objects.requireNonNull(l4FrontListener, "l4FrontListener"),
                Objects.requireNonNull(cluster, "cluster"),
                Objects.requireNonNull(coreConfiguration, "commonConfiguration")
        );
        l4Balance.cluster(cluster);
        l4FrontListener.l4LoadBalancer(defaultL4LoadBalancer);
        return defaultL4LoadBalancer;
    }
}
