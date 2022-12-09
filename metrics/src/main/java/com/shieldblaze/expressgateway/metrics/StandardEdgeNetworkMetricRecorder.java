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

import java.util.concurrent.atomic.AtomicLong;

/**
 * This class keeps track of Packets transmitted, Packets received,
 * Bandwidth transmitted and bandwidth received at edge (first in path) level.
 */
@ChannelHandler.Sharable
public final class StandardEdgeNetworkMetricRecorder extends ChannelDuplexHandler implements EdgeNetworkMetric {

    public static final StandardEdgeNetworkMetricRecorder INSTANCE = new StandardEdgeNetworkMetricRecorder();

    private final AtomicLong Bandwidth_TX = new AtomicLong();
    private final AtomicLong Bandwidth_RX = new AtomicLong();

    private final AtomicLong Packet_TX = new AtomicLong();
    private final AtomicLong Packet_RX = new AtomicLong();

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        Packet_RX.incrementAndGet();

        if (msg instanceof ByteBuf) {
            Bandwidth_RX.addAndGet(((ByteBuf) msg).readableBytes());
        } else if (msg instanceof ByteBufHolder) {
            Bandwidth_RX.addAndGet(((ByteBufHolder) msg).content().readableBytes());
        }

        ctx.fireChannelRead(msg);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        Packet_TX.incrementAndGet();

        if (msg instanceof ByteBuf) {
            Bandwidth_TX.addAndGet(((ByteBuf) msg).readableBytes());
        } else if (msg instanceof ByteBufHolder) {
            Bandwidth_TX.addAndGet(((ByteBufHolder) msg).content().readableBytes());
        }

        ctx.write(msg, promise);
    }

    /**
     * Get current Bandwidth transmitted
     */
    @Override
    public long bandwidthTX() {
        return Bandwidth_TX.getAndSet(0);
    }

    /**
     * Get current Bandwidth received
     */
    @Override
    public long bandwidthRX() {
        return Bandwidth_RX.getAndSet(0);
    }

    /**
     * Get current Packets transmitted
     */
    @Override
    public long packetTX() {
        return Packet_TX.getAndSet(0);
    }

    /**
     * Get current Packets received
     */
    @Override
    public long packetRX() {
        return Packet_RX.getAndSet(0);
    }

    @Override
    public void reset() {
        Bandwidth_TX.set(0L);
        Bandwidth_RX.set(0L);
        Packet_TX.set(0);
        Packet_RX.set(0);
    }

    private StandardEdgeNetworkMetricRecorder() {
        // Prevent outside initialization
    }
}
