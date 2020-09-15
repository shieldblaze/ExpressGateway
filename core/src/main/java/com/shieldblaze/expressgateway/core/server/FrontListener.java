package com.shieldblaze.expressgateway.core.server;

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.loadbalance.l4.L4Balance;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFuture;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;

public abstract class FrontListener {
    private static final Logger logger = LogManager.getLogger(FrontListener.class);

    protected InetSocketAddress bindAddress;
    protected List<ChannelFuture> channelFutureList = Collections.synchronizedList(new ArrayList<>());
    private final AtomicBoolean started = new AtomicBoolean(false);

    public FrontListener(InetSocketAddress bindAddress) {
        this.bindAddress = bindAddress;
    }

    public abstract void start(Configuration configuration, EventLoopFactory eventLoopFactory, ByteBufAllocator byteBufAllocator,
                               L4Balance l4Balance);

    public boolean waitForStart() {
        for (ChannelFuture channelFuture : channelFutureList) {
            try {
                started.set(channelFuture.sync().isSuccess());
                if (!started.get()) {
                    logger.error("Failed to Start FrontListener", channelFuture.cause());
                    stop();
                    break;
                }
            } catch (InterruptedException e) {
                logger.error("ChannelFuture Block Call was interrupted");
            }
        }

        return started.get();
    }

    public void stop() {
        for (ChannelFuture channelFuture : channelFutureList) {
            channelFuture.channel().closeFuture();
        }
        started.set(false);
    }

    public boolean isStarted() {
        return started.get();
    }
}
