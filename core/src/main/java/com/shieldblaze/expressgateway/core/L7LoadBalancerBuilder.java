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

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.server.L7FrontListener;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;
import io.netty.util.internal.ObjectUtil;

/**
 * Builder for {@link L7LoadBalancer}
 */
public final class L7LoadBalancerBuilder {
    private CommonConfiguration commonConfiguration;
    private HTTPConfiguration httpConfiguration;
    private L7Balance l7Balance;
    private Cluster cluster;
    private L7FrontListener l7FrontListener;

    private L7LoadBalancerBuilder() {
        // Prevent outside initialization
    }

    public static L7LoadBalancerBuilder newBuilder() {
        return new L7LoadBalancerBuilder();
    }

    public L7LoadBalancerBuilder withCommonConfiguration(CommonConfiguration commonConfiguration) {
        this.commonConfiguration = commonConfiguration;
        return this;
    }

    public L7LoadBalancerBuilder withHTTPConfiguration(HTTPConfiguration httpConfiguration) {
        this.httpConfiguration = httpConfiguration;
        return this;
    }

    public L7LoadBalancerBuilder withL7Balance(L7Balance l7Balance) {
        this.l7Balance = l7Balance;
        return this;
    }

    public L7LoadBalancerBuilder withCluster(Cluster cluster) {
        this.cluster = cluster;
        return this;
    }

    public L7LoadBalancerBuilder withL7FrontListener(L7FrontListener l7FrontListener) {
        this.l7FrontListener = l7FrontListener;
        return this;
    }

    public L7LoadBalancer build() {
        L7LoadBalancer l7LoadBalancer = new L7LoadBalancer();
        l7LoadBalancer.setCommonConfiguration(ObjectUtil.checkNotNull(commonConfiguration, "Common Configuration"));
        l7LoadBalancer.setHttpConfiguration(ObjectUtil.checkNotNull(httpConfiguration, "HTTP Configuration"));
        l7LoadBalancer.setL7Balance(ObjectUtil.checkNotNull(l7Balance, "L7 Balance"));
        l7LoadBalancer.setCluster(ObjectUtil.checkNotNull(cluster, "Cluster"));
        l7Balance.setBackends(cluster.getBackends());
        l7LoadBalancer.setL7FrontListener(ObjectUtil.checkNotNull(l7FrontListener, "L7 FrontListener"));
        return l7LoadBalancer;
    }
}
