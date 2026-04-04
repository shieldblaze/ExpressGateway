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
package com.shieldblaze.expressgateway.core.handlers;

import io.netty.channel.Channel;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;

/**
 * Propagates backpressure between a pair of channels (frontend <-> backend).
 *
 * <p>When this handler's channel becomes unwritable (outbound buffer exceeds the
 * high water mark), it disables autoRead on the paired channel, causing TCP/QUIC
 * to apply backpressure to the sender. When writability is restored, autoRead is
 * re-enabled on the paired channel.</p>
 *
 * <p>This handler should be added to BOTH the frontend and backend pipelines,
 * each referencing the other channel as the paired channel. This creates
 * bidirectional backpressure propagation:</p>
 * <ul>
 *   <li>Backend slow -> backend channel unwritable -> pause frontend reads</li>
 *   <li>Frontend slow -> frontend channel unwritable -> pause backend reads</li>
 * </ul>
 *
 * <p>Thread safety: The paired channel reference is set once at construction and
 * is immutable. The {@code channelWritabilityChanged} callback executes on this
 * channel's EventLoop. The {@code setAutoRead} call on the paired channel is
 * safe from any thread (Netty's ChannelConfig is thread-safe).</p>
 */
public final class BackpressureHandler extends ChannelDuplexHandler {

    private final Channel pairedChannel;

    /**
     * @param pairedChannel the channel whose autoRead will be toggled based on
     *                      this channel's writability
     */
    public BackpressureHandler(Channel pairedChannel) {
        this.pairedChannel = pairedChannel;
    }

    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        if (pairedChannel != null && pairedChannel.isActive()) {
            pairedChannel.config().setAutoRead(ctx.channel().isWritable());
        }
        super.channelWritabilityChanged(ctx);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        // When this channel goes down, resume reads on the paired channel
        // to prevent it from being stuck in a paused state indefinitely.
        if (pairedChannel != null && pairedChannel.isActive()) {
            pairedChannel.config().setAutoRead(true);
        }
        super.channelInactive(ctx);
    }
}
