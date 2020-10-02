package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.core.netty.PooledByteBufAllocatorBuffer;
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
                new PooledByteBufAllocatorBuffer(commonConfiguration.getPooledByteBufAllocatorConfiguration()).getInstance(),
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
