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
package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Balance;
import com.shieldblaze.expressgateway.core.server.FrontListener;
import io.netty.util.internal.ObjectUtil;

public final class L4LoadBalancerBuilder {
    private Configuration configuration;
    private L4Balance l4Balance;
    private Cluster cluster;
    private FrontListener frontListener;

    private L4LoadBalancerBuilder() {
        // Prevent outside initialization
    }

    public static L4LoadBalancerBuilder newBuilder() {
        return new L4LoadBalancerBuilder();
    }

    public L4LoadBalancerBuilder withConfiguration(Configuration configuration) {
        this.configuration = configuration;
        return this;
    }

    public L4LoadBalancerBuilder withL4Balance(L4Balance l4Balance) {
        this.l4Balance = l4Balance;
        return this;
    }

    public L4LoadBalancerBuilder withCluster(Cluster cluster) {
        this.cluster = cluster;
        return this;
    }

    public L4LoadBalancerBuilder withFrontListener(FrontListener frontListener) {
        this.frontListener = frontListener;
        return this;
    }

    public L4LoadBalancer build() {
        L4LoadBalancer l4LoadBalancer = new L4LoadBalancer();
        l4LoadBalancer.setConfiguration(ObjectUtil.checkNotNull(configuration, "Configuration"));
        l4LoadBalancer.setL4Balance(ObjectUtil.checkNotNull(l4Balance, "L4Balance"));
        l4Balance.setBackends(cluster.getBackends());
        l4LoadBalancer.setCluster(ObjectUtil.checkNotNull(cluster, "Cluster"));
        l4LoadBalancer.setFrontListener(ObjectUtil.checkNotNull(frontListener, "FrontListener"));
        return l4LoadBalancer;
    }
}
