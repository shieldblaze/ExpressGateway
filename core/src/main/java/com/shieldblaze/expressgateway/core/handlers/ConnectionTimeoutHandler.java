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

import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;

import java.time.Duration;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * {@linkplain ConnectionTimeoutHandler} is a {@linkplain ChannelDuplexHandler} that is used to handle Connection Timeout.
 * <p>
 * This Handler is used to handle Connection Timeout for both Upstream and Downstream Connections.
 * </p>
 *
 * <p><b>CM-F2 fix:</b> The periodic idle check is now scheduled on the channel's own
 * {@link io.netty.channel.EventLoop} instead of a global {@code ScheduledExecutorService}.
 * This eliminates cross-thread visibility issues (no volatile needed on timestamp fields),
 * avoids the overhead of marshalling events back to the EventLoop, and ensures the timer
 * is automatically cleaned up when the EventLoop shuts down.</p>
 *
 * <h3>IT-01: Mid-Request Idle Timeout Behavior</h3>
 * <p>The idle timeout is reset on every {@code channelRead()} event, which includes
 * each chunk of HTTP request body data during an upload. This means the idle timer
 * will <b>not</b> fire while a client is actively sending body data — each received
 * chunk resets the clock. The timer only fires if the client stops sending data
 * entirely for longer than the configured timeout duration.</p>
 *
 * <p>A client that pauses mid-upload beyond the timeout is treated as dead — this
 * is intentional and consistent with how Nginx ({@code client_body_timeout}) and
 * HAProxy ({@code timeout client}) handle stalled uploads. There is no need for
 * request-state-aware timeout logic: the per-read reset provides the correct
 * behavior for both idle connections and active transfers.</p>
 */
public final class ConnectionTimeoutHandler extends ChannelDuplexHandler {

    /**
     * Enum to represent the State of Connection Timeout
     */
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

    // CM-F2: Since both the timer callback and channelRead/write now execute on the
    // same EventLoop thread, volatile is no longer needed. Single-threaded access
    // guarantees visibility without memory barriers.
    private long lastTransferredRead = System.nanoTime();
    private long lastTransferredWrite = lastTransferredRead;
    private ScheduledFuture<?> scheduledFuture;

    /**
     * Create a new Instance of {@linkplain ConnectionTimeoutHandler}
     *
     * @param timeout    Timeout of Read/Write
     * @param isUpstream Set to {@code true} if this Instance is placed in Upstream Pipeline
     */
    public ConnectionTimeoutHandler(Duration timeout, boolean isUpstream) {
        timeoutNanos = timeout.toNanos();
        this.isUpstream = isUpstream;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        // CM-F2: Schedule the idle check on the channel's EventLoop. This runs on the
        // same thread as channelRead/write, so no cross-thread synchronization is needed.
        // The EventLoop's task queue handles cancellation on channel close automatically,
        // but we still cancel explicitly in channelInactive for deterministic cleanup.
        scheduledFuture = ctx.channel().eventLoop().scheduleAtFixedRate(
                () -> checkIdleTimeout(ctx), 500, 500, TimeUnit.MILLISECONDS);
        super.channelActive(ctx);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        if (scheduledFuture != null) {
            scheduledFuture.cancel(false);
            scheduledFuture = null;
        }
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

    /**
     * Checks whether read or write has been idle beyond the configured timeout.
     * This method runs on the channel's EventLoop thread, so all field accesses
     * are single-threaded and pipeline operations are safe without marshalling.
     */
    private void checkIdleTimeout(ChannelHandlerContext ctx) {
        if (!ctx.channel().isActive()) {
            return;
        }
        long nanoTime = System.nanoTime();
        boolean readIdle = nanoTime - lastTransferredRead > timeoutNanos;
        boolean writeIdle = nanoTime - lastTransferredWrite > timeoutNanos;

        if (readIdle) {
            ctx.fireUserEventTriggered(isUpstream ? State.UPSTREAM_READ_IDLE : State.DOWNSTREAM_READ_IDLE);
        }
        if (writeIdle) {
            ctx.fireUserEventTriggered(isUpstream ? State.UPSTREAM_WRITE_IDLE : State.DOWNSTREAM_WRITE_IDLE);
        }
    }
}
