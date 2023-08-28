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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.common.utils.ReferenceCountedUtil;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ConnectTimeoutException;

import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * <p> Base class for Connection. A connection is a Downstream {@link Channel}. Protocol implementations must extend this class. </p>
 *
 * <p> {@link #init(ChannelFuture)} must be called once {@link ChannelFuture} is ready
 * for a new connection. </p>
 */
public abstract class Connection {

    /**
     * Connection States
     */
    public enum State {
        /**
         * Connection has been initialized but not connected (or ready) yet.
         */
        INITIALIZED,

        /**
         * Connection has timed-out while connecting.
         */
        CONNECTION_TIMEOUT,

        /**
         * Connection has been closed with downstream channel.
         */
        CONNECTION_CLOSED,

        /**
         * Connection has been connected successfully, active and ready to accept traffic.
         */
        CONNECTED_AND_ACTIVE
    }

    /**
     * Backlog Queue contains objects pending to be written once connection establishes.
     * <p></p>
     * This queue will be {@code null} once backlog is processed (either via {@link #writeBacklog()} or {@link #clearBacklog()})
     */
    protected ConcurrentLinkedQueue<Object> backlogQueue;

    private final Node node;
    protected ChannelFuture channelFuture;
    protected Channel channel;
    protected InetSocketAddress socketAddress;
    protected State state = State.INITIALIZED;

    /**
     * Create a new {@link Connection} Instance
     *
     * @param node {@link Node} associated with this Connection
     */
    @NonNull
    protected Connection(Node node) {
        this.node = node;
        backlogQueue(new ConcurrentLinkedQueue<>());
    }

    /**
     * Initialize this Connection with {@link ChannelFuture}
     *
     * @param channelFuture {@link ChannelFuture} associated with this Connection (of Upstream or Downstream)
     */
    @NonNull
    public void init(ChannelFuture channelFuture) {
        if (this.channelFuture == null) {
            this.channelFuture = channelFuture;

            // Add listener to be notified when Channel initializes
            channelFuture.addListener(future -> {
                if (channelFuture.isSuccess()) {
                    state = State.CONNECTED_AND_ACTIVE;
                    socketAddress = (InetSocketAddress) channelFuture.channel().remoteAddress();
                    channel = channelFuture.channel();
                } else {
                    if (future.cause() instanceof ConnectTimeoutException) {
                        state = State.CONNECTION_TIMEOUT;
                    }
                }
                processBacklog(channelFuture); // Call Backlog Processor for Backlog Processing
            });

            // Add listener to be notified when Channel closes
            channelFuture.channel().closeFuture().addListener(future -> {
                node.removeConnection(this);
                state = State.CONNECTION_CLOSED;
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
     * Write Backlog to the {@link Channel}
     */
    protected void writeBacklog() {
        backlogQueue.forEach(this::writeAndFlush);
        backlogQueue = null;
    }

    /**
     * Clear the Backlog and release all objects.
     */
    protected void clearBacklog() {
        backlogQueue.forEach(ReferenceCountedUtil::silentRelease);
        backlogQueue = null;
    }

    /**
     * Write and Flush data
     *
     * @param o Data to be written
     */
    @NonNull
    public void writeAndFlush(Object o) {
        if (state == State.INITIALIZED) {
            backlogQueue.add(o);
        } else if (state == State.CONNECTED_AND_ACTIVE && channel != null) {
            channel.writeAndFlush(o);
        } else {
            ReferenceCountedUtil.silentRelease(o);
        }
    }

    /**
     * Get {@link ChannelFuture} associated with this connection.
     */
    public ChannelFuture channelFuture() {
        return channelFuture;
    }

    /**
     * Get {@link Node} associated with this connection.
     */
    public Node node() {
        return node;
    }

    /**
     * {@link InetSocketAddress} of this {@link Connection}
     */
    public InetSocketAddress socketAddress() {
        return socketAddress;
    }

    /**
     * Current {@link State} of this {@link Connection}
     */
    public State state() {
        return state;
    }

    /**
     * Set the {@link State} of this {@link Connection}
     */
    public void backlogQueue(ConcurrentLinkedQueue<Object> newQueue) {
        backlogQueue = newQueue;
    }

    /**
     * Close this {@link Connection}
     */
    public synchronized void close() {
        // If Backlog Queue contains something then clear it before closing connection.
        if (!backlogQueue.isEmpty()) {
            clearBacklog();
        }

        // Remove this connection from Node
        node.removeConnection(this);

        // If Channel is not null then close it.
        // Channel can be null if the connection is not initialized.
        if (channel != null) {
            channel.close();
        }
    }

    @Override
    public String toString() {
        return '{' + "node=" + node + ", socketAddress=" + socketAddress + ", state=" + state + '}';
    }
}
