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
package com.shieldblaze.expressgateway.core.server;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Balance;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.EventLoopGroup;
import io.netty.util.internal.ObjectUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;

public abstract class FrontListener {
    private static final Logger logger = LogManager.getLogger(FrontListener.class);

    protected final InetSocketAddress bindAddress;
    protected final List<ChannelFuture> channelFutureList = Collections.synchronizedList(new ArrayList<>());
    private final AtomicBoolean started = new AtomicBoolean(false);

    public FrontListener(InetSocketAddress bindAddress) {
        this.bindAddress = ObjectUtil.checkNotNull(bindAddress, "Bind Address");
    }

    /**
     * Start a {@link FrontListener}
     * @param commonConfiguration {@link CommonConfiguration} to be applied
     * @param eventLoopFactory {@link EventLoopFactory} for {@link EventLoopGroup}
     * @param byteBufAllocator {@link ByteBufAllocator} for {@link ByteBuf} allocation
     * @param l4Balance {@link L4Balance} for load-balance
     */
    public abstract void start(CommonConfiguration commonConfiguration, EventLoopFactory eventLoopFactory, ByteBufAllocator byteBufAllocator,
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
