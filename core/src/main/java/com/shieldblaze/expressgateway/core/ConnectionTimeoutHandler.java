/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;

import java.time.Duration;

public class ConnectionTimeoutHandler extends ChannelDuplexHandler {

    private final long timeoutNanos;
    private long lastTransferred = System.nanoTime();

    public ConnectionTimeoutHandler(Duration timeout) {
        this.timeoutNanos = timeout.toNanos();
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        check(ctx);
        super.channelRead(ctx, msg);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        check(ctx);
        super.write(ctx, msg, promise);
    }

    private void check(ChannelHandlerContext ctx) {
        long nanoTime = System.nanoTime();
        if (nanoTime - lastTransferred > timeoutNanos) {
            ctx.close();
        } else {
            lastTransferred = nanoTime;
        }
    }
}
