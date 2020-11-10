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
package com.shieldblaze.expressgateway.backend.pool;

import com.shieldblaze.expressgateway.common.utils.ReferenceCounted;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelPromise;

import java.util.concurrent.ConcurrentLinkedQueue;

public class Connection {

    /**
     * Backlog Queue contains objects pending to be written once connection establishes.
     */
    private ConcurrentLinkedQueue<Backlog> backlogQueue = new ConcurrentLinkedQueue<>();

    private final ChannelFuture channelFuture;
    private boolean inUse;

    public Connection(ChannelFuture channelFuture) {
        this.inUse = true;
        this.channelFuture = channelFuture;

        // Add listener to handle Backlog once connection is established.
        this.channelFuture.addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                ConcurrentLinkedQueue<Backlog> queue = new ConcurrentLinkedQueue<>(backlogQueue); // Make copy of Queue
                backlogQueue = null; // Make old queue null so no more data is written to it.
                queue.forEach(backlog -> future.channel().writeAndFlush(backlog.getObject(), backlog.getChannelPromise()));
                queue.clear(); // Clear the new queue because we're done with it.
            } else {
                backlogQueue.forEach(backlog -> {
                    ReferenceCounted.silentRelease(backlog.getObject());
                    backlog.getChannelPromise().tryFailure(future.cause());
                });
                backlogQueue = null;
            }
        });

        // Add listener to be notified when Channel closes
        this.channelFuture.channel().closeFuture().addListener((ChannelFutureListener) future -> inUse = false);
    }

    /**
     * Write and Flush data
     * @param o Data to be flushed
     * @return {@link ChannelFuture} of this write and flush
     */
    public ChannelFuture writeAndFlush(Object o) {
        return writeAndFlush(o, channelFuture.channel().newPromise());
    }

    /**
     * Write and Flush data
     * @param o Data to be flushed
     * @param channelPromise {@link ChannelFuture} to use
     * @return {@link ChannelFuture} of this write and flush
     */
    public ChannelPromise writeAndFlush(Object o, ChannelPromise channelPromise) {
        if (channelFuture.channel().isActive()) {
            channelFuture.channel().writeAndFlush(o, channelPromise);
            return channelPromise;
        } else if (backlogQueue != null) {
            Backlog backlog = new Backlog(o, channelPromise);
            backlogQueue.add(backlog);
            return channelPromise;
        } else {
            ReferenceCounted.silentRelease(o);
            channelPromise.tryFailure(new IllegalArgumentException("Connection is not accepting data anymore"));
            return null;
        }
    }

    /**
     * Lease this {@linkplain Connection} for use.
     *
     * @return {@linkplain Connection} Instance
     * @throws IllegalAccessException If this connection is already in use
     */
    public Connection lease() throws IllegalAccessException {
        if (inUse) {
            throw new IllegalAccessException("Connection is in use.");
        } else {
            inUse = true;
            return this;
        }
    }

    /**
     * Release this {@linkplain Connection} to be used by others.
     *
     * @return {@linkplain Connection} Instance
     * @throws IllegalArgumentException If this connection is already released
     */
    public Connection release() throws IllegalArgumentException {
        if (!inUse) {
            throw new IllegalArgumentException("Connection is already released.");
        } else {
            inUse = false;
            return this;
        }
    }

    /**
     * Check if this {@linkplain Connection} is in use.
     *
     * @return Returns {@code true} if this {@linkplain Connection} is in use
     * else set to {@code false}.
     */
    public boolean isInUse() {
        return inUse;
    }

    /**
     * Check if this {@linkplain Connection} is active and connected.
     * @return Returns {@code true} if this {@linkplain Connection} is active and connected
     * else set to {@code false}.
     */
    public boolean isActive() {
        return channelFuture.channel().isActive();
    }

    /**
     * Close this {@linkplain Connection}
     */
    public void close() {
        channelFuture.channel().close();
    }
}
