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
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.core.netty.PooledByteBufAllocator;
import com.shieldblaze.expressgateway.core.server.L7FrontListener;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;

/**
 * Layer-7 Load Balancer
 */
public class L7LoadBalancer {

    private CommonConfiguration commonConfiguration;
    private HTTPConfiguration httpConfiguration;
    private L7Balance l7Balance;
    private Cluster cluster;
    private L7FrontListener l7FrontListener;
    private EventLoopFactory eventLoopFactory;

    public boolean start() {
        eventLoopFactory = new EventLoopFactory(commonConfiguration);
        l7FrontListener.start(commonConfiguration, eventLoopFactory,
                new PooledByteBufAllocator(commonConfiguration.getPooledByteBufAllocatorConfiguration()).getInstance(),
                httpConfiguration,
                l7Balance);
        return l7FrontListener.waitForStart();
    }

    public void stop() {
        l7FrontListener.stop();
        eventLoopFactory.getParentGroup().shutdownGracefully();
        eventLoopFactory.getChildGroup().shutdownGracefully();
    }

    void setCommonConfiguration(CommonConfiguration commonConfiguration) {
        this.commonConfiguration = commonConfiguration;
    }

    void setHttpConfiguration(HTTPConfiguration httpConfiguration) {
        this.httpConfiguration = httpConfiguration;
    }

    void setL7Balance(L7Balance l7Balance) {
        this.l7Balance = l7Balance;
    }

    void setCluster(Cluster cluster) {
        this.cluster = cluster;
    }

    void setL7FrontListener(L7FrontListener l7FrontListener) {
        this.l7FrontListener = l7FrontListener;
    }

    public Cluster getCluster() {
        return cluster;
    }
}
