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
package com.shieldblaze.expressgateway.security;

import io.github.bucket4j.Bandwidth;
import io.github.bucket4j.Bucket;
import io.github.bucket4j.Bucket4j;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.socket.DatagramPacket;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;

/**
 * {@link PacketRateLimit} Rate-Limits Packets per Connection
 */
public final class PacketRateLimit extends ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(PacketRateLimit.class);

    private final Bucket bucket;

    /**
     * Create a new {@link PacketRateLimit} Instance
     *
     * @param packet   Number of packet for Rate-Limit
     * @param duration {@link Duration} of Rate-Limit
     */
    public PacketRateLimit(int packet, Duration duration) {
        Bandwidth limit = Bandwidth.simple(packet, duration);
        bucket = Bucket4j.builder().addLimit(limit).withNanosecondPrecision().build();
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (bucket.asAsync().tryConsume(1).get()) {
            super.channelRead(ctx, msg);
        } else {
            if (msg instanceof ByteBuf) {
                ((ByteBuf) msg).release();
                logger.debug("Rate-Limit exceeded, Denying new Packet from {}", ctx.channel().remoteAddress());
            } else if (msg instanceof DatagramPacket) {
                ((DatagramPacket) msg).release();
                logger.debug("Rate-Limit exceeded, Denying new Packet from {}", ((DatagramPacket) msg).sender());
            }
        }
    }
}
