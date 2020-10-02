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
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Balance;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.core.netty.PooledByteBufAllocatorBuffer;
import com.shieldblaze.expressgateway.core.server.FrontListener;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;

/**
 * Layer-4 Load Balancer
 */
public final class L4LoadBalancer {

    L4LoadBalancer() {
        // Prevent outside initialization
    }

    private CommonConfiguration commonConfiguration;
    private L4Balance l4Balance;
    private L7Balance l7Balance;
    private Cluster cluster;
    private FrontListener frontListener;
    private EventLoopFactory eventLoopFactory;

    public boolean start() {
        eventLoopFactory = new EventLoopFactory(commonConfiguration);
//        frontListener.start(commonConfiguration, eventLoopFactory,
//                new PooledByteBufAllocatorBuffer(commonConfiguration.getPooledByteBufAllocatorConfiguration()).getInstance(), l4Balance);
        frontListener.start(commonConfiguration, eventLoopFactory,
                new PooledByteBufAllocatorBuffer(commonConfiguration.getPooledByteBufAllocatorConfiguration()).getInstance(), l7Balance);
        return frontListener.waitForStart();
    }

    public void stop() {
        frontListener.stop();
        eventLoopFactory.getParentGroup().shutdownGracefully();
        eventLoopFactory.getChildGroup().shutdownGracefully();
    }

    public boolean hasStarted() {
        return frontListener.isStarted();
    }

    void setConfiguration(CommonConfiguration commonConfiguration) {
        this.commonConfiguration = commonConfiguration;
    }

    void setL4Balance(L4Balance l4Balance) {
        this.l4Balance = l4Balance;
    }

    void setL7Balance(L7Balance l7Balance) {
        this.l7Balance = l7Balance;
    }

    void setCluster(Cluster cluster) {
        this.cluster = cluster;
    }

    void setFrontListener(FrontListener frontListener) {
        this.frontListener = frontListener;
    }

    public Cluster getCluster() {
        return cluster;
    }
}
