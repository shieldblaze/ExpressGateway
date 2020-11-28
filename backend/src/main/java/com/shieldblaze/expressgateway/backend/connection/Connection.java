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

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.common.utils.ReferenceCounted;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelPromise;

import java.net.InetSocketAddress;
import java.time.Instant;
import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * <p> Base class for Connection. Protocol implementations must extend this class. </p>
 *
 * <p> {@link #init(ChannelFuture)} must be called once {@link ChannelFuture} is ready
 * for a new connection. </p>
 */
public abstract class Connection {

    /**
     * Backlog Queue contains objects pending to be written once connection establishes.
     */
    protected ConcurrentLinkedQueue<Backlog> backlogQueue = new ConcurrentLinkedQueue<>();

    private final long timeout;
    private ChannelFuture channelFuture;
    private boolean inUse;
    private InetSocketAddress socketAddress;

    public Connection(long timeout) {
        this.timeout = Instant.now().plusMillis(timeout).toEpochMilli();
    }

    /**
     * Initialize this Connection
     */
    @NonNull
    public void init(ChannelFuture channelFuture) {
        if (this.channelFuture == null) {
            this.channelFuture = channelFuture;

            // Add listener to be notified when Channel initializes
            this.channelFuture.addListener((ChannelFutureListener) future -> {
                processBacklog(channelFuture);
                if (channelFuture.isSuccess()) {
                    socketAddress = (InetSocketAddress) channelFuture.channel().remoteAddress();
                }
            });

            // Add listener to be notified when Channel closes
            this.channelFuture.channel()
                    .closeFuture()
                    .addListener((ChannelFutureListener) future -> inUse = false);
        } else {
            throw new IllegalArgumentException("Connection is already initialized");
        }
    }

    /**
     * This method is called when {@link #channelFuture()} has finished the operation.
     * Protocol implementations extending this class must clear {@link #backlogQueue} when
     * this method is called.
     */
    protected abstract void processBacklog(ChannelFuture channelFuture);

    /**
     * Write and Process the Backlog
     */
    @NonNull
    protected void writeBacklog(ChannelFuture channelFuture) {
        ConcurrentLinkedQueue<Backlog> queue = new ConcurrentLinkedQueue<>(backlogQueue); // Make copy of Queue
        backlogQueue = null; // Make old queue null so no more data is written to it.
        queue.forEach(backlog -> channelFuture.channel().writeAndFlush(backlog.object(), backlog.channelPromise()));
        queue.clear(); // Clear the new queue because we're done with it.
    }

    /**
     * Clear the Backlog and release all objects.
     */
    @NonNull
    protected void clearBacklog(Throwable throwable) {
        backlogQueue.forEach(backlog -> {
            ReferenceCounted.silentRelease(backlog.object());
            backlog.channelPromise().tryFailure(throwable);
        });
        backlogQueue = null;
    }

    /**
     * Write and Flush data
     *
     * @param o Data to be written
     * @return {@link ChannelFuture} of this write and flush
     */
    public ChannelFuture writeAndFlush(Object o) {
        return writeAndFlush(o, channelFuture.channel().newPromise());
    }

    /**
     * Write and Flush data
     *
     * @param o              Data to be written
     * @param channelPromise {@link ChannelFuture} to use
     * @return {@link ChannelFuture} of this write and flush
     */
    public ChannelPromise writeAndFlush(Object o, ChannelPromise channelPromise) {
        if (backlogQueue != null) {
            Backlog backlog = new Backlog(o, channelPromise);
            backlogQueue.add(backlog);
            return channelPromise;
        } else if (channelFuture.channel().isActive()) {
            channelFuture.channel().writeAndFlush(o, channelPromise);
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
     *
     * @return Returns {@code true} if this {@linkplain Connection} is active and connected
     * else set to {@code false}.
     */
    public boolean isActive() {
        return channelFuture.channel().isActive();
    }

    /**
     * Check if this connection timed out
     *
     * @return {@code true} if this connection timed out else {@code false}
     */
    public boolean hasConnectionTimedOut() {
        return System.currentTimeMillis() > timeout;
    }

    /**
     * Get {@link ChannelFuture} associated with this connection.
     */
    public ChannelFuture channelFuture() {
        return channelFuture;
    }

    public InetSocketAddress socketAddress() {
        return socketAddress;
    }

    /**
     * Close this {@linkplain Connection}
     */
    public void close() {
        channelFuture.channel().close();
    }

    @Override
    public String toString() {
        return "Connection{" +
                "timeout=" + timeout +
                ", channelFuture=" + channelFuture +
                ", inUse=" + inUse +
                ", socketAddress=" + socketAddress +
                '}';
    }
}
