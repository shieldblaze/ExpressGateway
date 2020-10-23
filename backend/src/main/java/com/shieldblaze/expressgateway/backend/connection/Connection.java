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
package com.shieldblaze.expressgateway.backend.connection;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.common.utils.ReferenceCounted;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelPipeline;

import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * Connection to a {@linkplain Backend}
 */
public class Connection {

    protected ChannelFuture channelFuture;
    private ConcurrentLinkedQueue<Object> backlogQueue = new ConcurrentLinkedQueue<>();
    private boolean inUse = true;
    protected State state = State.NEW;

    public void setChannelFuture(ChannelFuture channelFuture) {
        if (this.channelFuture == null) {
            this.channelFuture = channelFuture;

            // Add Listener to write all pending backlog data.
            this.channelFuture.addListener((ChannelFutureListener) future -> {
                if (future.isSuccess()) {
                    state = State.ESTABLISHED;
                } else {
                    state = State.CLOSED;
                }
                releaseBacklog(future.isSuccess());
            });

            this.channelFuture.channel().closeFuture().addListener((ChannelFutureListener) future -> {
               state = State.CLOSED;
            });
        } else {
            throw new IllegalArgumentException("ChannelFuture is already set");
        }
    }

    /**
     * Check if connection is active and connected.
     *
     * @return {@code true} if connection is active and connected else {@code false}
     */
    public boolean isActive() {
        return channelFuture.channel().isActive();
    }

    /**
     * Check if connection is in use.
     *
     * @return {@code true} if connection is in use else {@code false}
     */
    public boolean isInUse() {
        return inUse;
    }

    /**
     * Set {@code inUse} property
     *
     * @param inUse Set to {@code true} if connection is in use and should not be acquired
     *              else set to {@code false}
     */
    public void setInUse(boolean inUse) {
        this.inUse = inUse;
    }

    /**
     * Write and Flush message
     */
    public void writeAndFlush(Object msg) {
        // - If BacklogQueue is not null then connection is not established yet
        // then we'll add message to backlog.
        // - If BacklogQueue is null and channel is active then write the message.
        // - If both case are not matched then connection is not active and we'll release the message.
        if (backlogQueue != null) {
            backlogQueue.add(msg);
            return;
        } else if (channelFuture.channel().isActive()) {
           channelFuture.channel().writeAndFlush(msg);
           return;
        }
        ReferenceCounted.silentFullRelease(msg);
    }

    public ChannelFuture channelFuture() {
        return channelFuture;
    }

    protected void releaseBacklog(boolean write) {
        if (write) {
            backlogQueue.forEach(o -> channelFuture.channel().writeAndFlush(o));
        } else {
            backlogQueue.forEach(ReferenceCounted::silentFullRelease);
        }
        backlogQueue = null;
    }

    public State getState() {
        return state;
    }

    public enum State {
        NEW,
        ESTABLISHED,
        CLOSED
    }
}
