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
package com.shieldblaze.expressgateway.core.loadbalancer.l4;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Balance;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for creating default {@link L4LoadBalancer}
 */
public final class L4LoadBalancerBuilder {
    private InetSocketAddress bindAddress;
    private CommonConfiguration commonConfiguration;
    private L4Balance l4Balance;
    private Cluster cluster;
    private L4FrontListener l4FrontListener;

    private L4LoadBalancerBuilder() {
        // Prevent outside initialization
    }

    /**
     * Create a new {@link L4LoadBalancerBuilder} Instance
     *
     * @return {@link L4LoadBalancerBuilder} Instance
     */
    public static L4LoadBalancerBuilder newBuilder() {
        return new L4LoadBalancerBuilder();
    }

    /**
     * Set {@link CommonConfiguration}
     */
    public L4LoadBalancerBuilder withCommonConfiguration(CommonConfiguration commonConfiguration) {
        this.commonConfiguration = commonConfiguration;
        return this;
    }

    /**
     * Set {@link L4Balance} Type
     */
    public L4LoadBalancerBuilder withL4Balance(L4Balance l4Balance) {
        this.l4Balance = l4Balance;
        return this;
    }

    /**
     * Set {@link Cluster} to Load Balance
     */
    public L4LoadBalancerBuilder withCluster(Cluster cluster) {
        this.cluster = cluster;
        return this;
    }

    /**
     * Set {@link L4FrontListener} to listen incoming traffic
     */
    public L4LoadBalancerBuilder withFrontListener(L4FrontListener l4FrontListener) {
        this.l4FrontListener = l4FrontListener;
        return this;
    }

    /**
     * Set {@link InetSocketAddress} where {@link L4FrontListener} will bind and listen
     */
    public L4LoadBalancerBuilder withBindAddress(InetSocketAddress bindAddress) {
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
                Objects.requireNonNull(commonConfiguration, "commonConfiguration")
        );
        l4Balance.setCluster(cluster);
        l4FrontListener.setL4LoadBalancer(defaultL4LoadBalancer);
        return defaultL4LoadBalancer;
    }
}
