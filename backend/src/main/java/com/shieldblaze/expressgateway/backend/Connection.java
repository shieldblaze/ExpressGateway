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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.common.utils.ReferenceCounted;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;

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
    protected ConcurrentLinkedQueue<Object> backlogQueue = new ConcurrentLinkedQueue<>();

    private final Node node;
    private final long timeout;
    private ChannelFuture channelFuture;
    private Channel channel;
    private InetSocketAddress socketAddress;
    private boolean isActive;

    public Connection(Node node, long timeout) {
        this.node = node;
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
            this.channelFuture.addListener(future -> {
                if (channelFuture.isSuccess()) {
                    socketAddress = (InetSocketAddress) channelFuture.channel().remoteAddress();
                    isActive = true;
                    channel = channelFuture.channel();
                }
                processBacklog(channelFuture);
            });

            // Add listener to be notified when Channel closes
            this.channelFuture.channel().closeFuture().addListener(future -> {
                isActive = false;
                node.removeConnection(this);
            });
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
    protected void writeBacklog() {
        ConcurrentLinkedQueue<Object> queue = new ConcurrentLinkedQueue<>(backlogQueue); // Make copy of Queue
        backlogQueue = null; // Make old queue null so no more data is written to it.
        queue.forEach(this::writeAndFlush);
        queue.clear(); // Clear the new queue because we're done with it.
    }

    /**
     * Clear the Backlog and release all objects.
     */
    protected void clearBacklog() {
        backlogQueue.forEach(ReferenceCounted::silentRelease);
        backlogQueue = null;
    }

    /**
     * Write and Flush data
     *
     * @param o Data to be written
     * @return {@link ChannelFuture} of this write and flush
     */
    @NonNull
    public void writeAndFlush(Object o) {
        if (backlogQueue != null) {
            backlogQueue.add(o);
        } else if (isActive) {
            channel.writeAndFlush(o, channel.voidPromise());
        } else {
            ReferenceCounted.silentRelease(o);
        }
    }

    /**
     * Release this {@linkplain Connection} back to connection pool.
     */
    public Connection release() {
        node.release0(this);
        return this;
    }

    /**
     * Check if this {@linkplain Connection} is active and connected.
     *
     * @return Returns {@code true} if this {@linkplain Connection} is active and connected
     * else set to {@code false}.
     */
    public boolean isActive() {
        return isActive;
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

    public Node node() {
        return node;
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
                "node=" + node +
                ", socketAddress=" + socketAddress +
                ", isActive=" + isActive +
                '}';
    }
}
