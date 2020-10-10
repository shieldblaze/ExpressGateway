package com.shieldblaze.expressgateway.backend.connection;

import com.shieldblaze.expressgateway.backend.Backend;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import stormpot.Poolable;

import java.util.concurrent.ConcurrentLinkedQueue;

public abstract class Connection<T> implements Poolable {

    /**
     * Backlog of Data
     */
    private final ConcurrentLinkedQueue<T> backlogQueue = new ConcurrentLinkedQueue<>();

    /**
     * Max Data Backlog Size
     */
    private final int maxDataBacklog;

    public Connection(int maxDataBacklog) {
        this.maxDataBacklog = maxDataBacklog;
    }

    /**
     * Set to {@code true} if Connection is connected.
     */
    protected boolean isConnected;

    /**
     * {@link Backend} linked with this {@link Connection}
     */
    protected Backend backend;

    /**
     * {@link ConnectionBootstrap} Instance
     */
    protected ConnectionBootstrap connectionBootstrap;

    /**
     * {@link Channel} associated with this Connection.
     */
    protected Channel channel;

    /**
     * Set to {@code true} if this Connection is already in use.
     */
    protected boolean inUse;

    protected ChannelFuture writeData(T data) {
        // If Connection is already established, write the data.
        if (isConnected) {
            return channel.writeAndFlush(data);
        } else {
            // If Connection is not already established, establish a new connection.
            connect();

            // If Backlog Queue size is smaller then Max Backlog then add the data to backlog.
            if (backlogQueue.size() < maxDataBacklog) {
                backlogQueue.add(data);
                return null;
            }
        }

        if (data instanceof ByteBuf) {
            ((ByteBuf) data).release();
        }

        return null;
    }

    /**
     * Establish a connection to {@link Backend}
     */
    public void connect() {
        ChannelFuture channelFuture = connectionBootstrap.bootstrap().connect(backend.getSocketAddress()).addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                isConnected = true;
                ConcurrentLinkedQueue<T> _backingQueue = new ConcurrentLinkedQueue<>(backlogQueue);
                backlogQueue.clear();

                _backingQueue.forEach(data -> future.channel().writeAndFlush(data));
            } else {
                isConnected = false;
                backlogQueue.clear();
            }
        });

        channel = channelFuture.channel();
        channel.closeFuture().addListener((ChannelFutureListener) future -> {
           isConnected = false;
        });
    }

    /**
     * Disconnect from {@link Backend}
     */
    public void disconnect() {
        if (isConnected) {
            isConnected = false;
            channel.close();
        }
    }

    @Override
    public void release() {
        inUse = false;
    }

    public boolean isInUse() {
        return inUse;
    }

    public boolean isConnected() {
        return isConnected;
    }
}
