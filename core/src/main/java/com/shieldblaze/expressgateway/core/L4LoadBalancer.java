package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.core.loadbalance.l4.L4Balance;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.core.netty.PooledByteBufAllocatorBuffer;
import com.shieldblaze.expressgateway.core.server.FrontListener;

public final class L4LoadBalancer {

    L4LoadBalancer() {
        // Prevent outside initialization
    }

    private Configuration configuration;
    private L4Balance l4Balance;
    private Cluster cluster;
    private FrontListener frontListener;

    public boolean start() {
        frontListener.start(configuration, new EventLoopFactory(configuration),
                new PooledByteBufAllocatorBuffer(configuration.getPooledByteBufAllocatorConfiguration()).getInstance(), l4Balance);
        return frontListener.waitForStart();
    }

    void setConfiguration(Configuration configuration) {
        this.configuration = configuration;
    }

    void setL4Balance(L4Balance l4Balance) {
        this.l4Balance = l4Balance;
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
