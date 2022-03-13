/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.metrics;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufHolder;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;

import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * This class keeps track of Packets transmitted, Packets received,
 * Bandwidth transmitted and bandwidth received at edge (first in path) level.
 */
@ChannelHandler.Sharable
public final class EdgeNetworkMetricRecorder extends ChannelDuplexHandler implements EdgeNetworkMetric {

    public static final EdgeNetworkMetricRecorder INSTANCE = new EdgeNetworkMetricRecorder();

    private final AtomicLong bandwidthTX = new AtomicLong();
    private final AtomicLong bandwidthRX = new AtomicLong();

    private final AtomicInteger packetTX = new AtomicInteger();
    private final AtomicInteger packetRX = new AtomicInteger();

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        packetRX.incrementAndGet();

        if (msg instanceof ByteBuf) {
            bandwidthRX.addAndGet(((ByteBuf) msg).readableBytes());
        } else if (msg instanceof ByteBufHolder) {
            bandwidthRX.addAndGet(((ByteBufHolder) msg).content().readableBytes());
        }

        ctx.fireChannelRead(msg);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        packetTX.incrementAndGet();

        if (msg instanceof ByteBuf) {
            bandwidthTX.addAndGet(((ByteBuf) msg).readableBytes());
        } else if (msg instanceof ByteBufHolder) {
            bandwidthTX.addAndGet(((ByteBufHolder) msg).content().readableBytes());
        }

        ctx.write(msg, promise);
    }

    /**
     * Get current Bandwidth transmitted
     */
    @Override
    public long bandwidthTX() {
        return bandwidthTX.getAndSet(0);
    }

    /**
     * Get current Bandwidth received
     */
    @Override
    public long bandwidthRX() {
        return bandwidthRX.getAndSet(0);
    }

    /**
     * Get current Packets transmitted
     */
    @Override
    public int packetTX() {
        return packetTX.getAndSet(0);
    }

    /**
     * Get current Packets received
     */
    @Override
    public int packetRX() {
        return packetRX.getAndSet(0);
    }

    /**
     * Reset all metrics counter
     */
    public void resetMetrics() {
        bandwidthTX.set(0L);
        bandwidthRX.set(0L);
        packetTX.set(0);
        packetRX.set(0);
    }

    private EdgeNetworkMetricRecorder() {
        // Prevent outside initialization
    }
}
