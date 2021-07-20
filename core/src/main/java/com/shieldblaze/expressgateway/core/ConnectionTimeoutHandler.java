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

import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;

import java.time.Duration;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

public final class ConnectionTimeoutHandler extends ChannelDuplexHandler implements Runnable {

    public enum State {
        /**
         * When Upstream Read(Receiving) is Idle
         */
        UPSTREAM_READ_IDLE,

        /**
         * When Upstream Write(Sending) is Idle
         */
        UPSTREAM_WRITE_IDLE,

        /**
         * When Downstream Read(Receiving) is Idle
         */
        DOWNSTREAM_READ_IDLE,

        /**
         * When Downstream Write(Sending) is Idle
         */
        DOWNSTREAM_WRITE_IDLE,
    }

    private final long timeoutNanos;
    private final boolean isUpstream;
    private long lastTransferredRead = System.nanoTime();
    private long lastTransferredWrite = System.nanoTime();
    private ChannelHandlerContext ctx;
    private ScheduledFuture<?> scheduledFuture;

    /**
     * Create a new Instance of {@linkplain ConnectionTimeoutHandler}
     *
     * @param timeout    Timeout of Read/Write
     * @param isUpstream Set to {@code true} if this Instance is placed in Upstream Pipeline
     */
    public ConnectionTimeoutHandler(Duration timeout, boolean isUpstream) {
        this.timeoutNanos = timeout.toNanos();
        this.isUpstream = isUpstream;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        this.ctx = ctx;
        scheduledFuture = GlobalExecutors.submitTaskAndRunEvery(this, 0, 500, TimeUnit.MILLISECONDS);
        super.channelActive(ctx);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        scheduledFuture.cancel(true);
        super.channelInactive(ctx);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        lastTransferredRead = System.nanoTime();
        super.channelRead(ctx, msg);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        lastTransferredWrite = System.nanoTime();
        super.write(ctx, msg, promise);
    }

    @Override
    public void run() {
        long nanoTime = System.nanoTime();

        if (nanoTime - lastTransferredRead > timeoutNanos) {
            ctx.fireUserEventTriggered(isUpstream ? State.UPSTREAM_READ_IDLE : State.DOWNSTREAM_READ_IDLE);
        }

        if (nanoTime - lastTransferredWrite > timeoutNanos) {
            ctx.fireUserEventTriggered(isUpstream ? State.UPSTREAM_WRITE_IDLE : State.DOWNSTREAM_WRITE_IDLE);
        }
    }
}
