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
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ConnectTimeoutException;

import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.BooleanSupplier;

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
     * Maximum number of pending messages allowed in the backlog queue.
     * Prevents OOM when the backend connection is slow to establish and
     * the client keeps sending data. ConcurrentLinkedQueue.size() is O(n),
     * so we track the count separately with an AtomicInteger for O(1) checks.
     */
    private static final int DEFAULT_MAX_BACKLOG_SIZE = 10_000;

    /**
     * Backlog Queue contains objects pending to be written once connection establishes.
     * <p></p>
     * This queue will be {@code null} once backlog is processed (either via {@link #writeBacklog()} or {@link #clearBacklog()})
     */
    protected ConcurrentLinkedQueue<Object> backlogQueue;

    /**
     * O(1) counter tracking the number of items in {@link #backlogQueue}.
     * Using AtomicInteger avoids the O(n) traversal of ConcurrentLinkedQueue.size().
     * Protected so subclasses that override writeBacklog() can reset it.
     */
    protected final AtomicInteger backlogSize = new AtomicInteger(0);

    /**
     * MEM-02: Reduced backlog limit when memory pressure is detected. Instead of
     * allowing the full maxBacklogSize messages, we shed load earlier to prevent
     * the JVM from running out of direct memory.
     */
    private static final int MEMORY_PRESSURE_BACKLOG_RATIO_DIVISOR = 4;

    // WS-F1: Protected so WebSocketConnection can enforce the same backlog bound
    // in its overridden writeAndFlush(). Keep private if no subclass needs it.
    protected final int maxBacklogSize;

    /**
     * MEM-02: Optional memory pressure callback. When non-null and returns {@code true},
     * the effective backlog limit is reduced to {@code maxBacklogSize / 4} to shed load
     * earlier and prevent OOM. Set via {@link #setMemoryPressureSupplier(BooleanSupplier)}.
     * The supplier is called on every backlog add, so it must be fast (no blocking, no allocation).
     */
    private volatile BooleanSupplier memoryPressureSupplier;

    private final Node node;
    protected ChannelFuture channelFuture;
    protected Channel channel;
    protected InetSocketAddress socketAddress;

    /**
     * Returns the underlying Netty {@link Channel} for this connection,
     * or {@code null} if the connection has not yet been established.
     */
    public Channel channel() {
        return channel;
    }
    protected volatile State state = State.INITIALIZED;

    /**
     * Create a new {@link Connection} Instance with default max backlog size.
     *
     * @param node {@link Node} associated with this Connection
     */
    @NonNull
    protected Connection(Node node) {
        this(node, DEFAULT_MAX_BACKLOG_SIZE);
    }

    /**
     * Create a new {@link Connection} Instance with configurable max backlog size.
     *
     * @param node           {@link Node} associated with this Connection
     * @param maxBacklogSize Maximum number of pending messages allowed in the backlog queue
     */
    @NonNull
    protected Connection(Node node, int maxBacklogSize) {
        this.node = node;
        this.maxBacklogSize = maxBacklogSize;
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
                    // MED-40: Set channel before processBacklog so backlog can write,
                    // but keep state as INITIALIZED until backlog is drained.
                    // This prevents writeAndFlush() from bypassing the backlog queue
                    // during the INITIALIZED->CONNECTED transition.
                    socketAddress = (InetSocketAddress) channelFuture.channel().remoteAddress();
                    channel = channelFuture.channel();
                } else {
                    if (future.cause() instanceof ConnectTimeoutException) {
                        state = State.CONNECTION_TIMEOUT;
                    }
                }
                processBacklog(channelFuture); // Call Backlog Processor for Backlog Processing
                // MED-40: Set state to CONNECTED_AND_ACTIVE after backlog is processed,
                // so new writes during backlog drain are still queued rather than racing.
                if (channelFuture.isSuccess()) {
                    state = State.CONNECTED_AND_ACTIVE;
                }
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
     * Number of messages to drain from the backlog per batch before checking
     * channel writability. Keeps the outbound buffer from growing unbounded
     * when thousands of messages have accumulated during connection setup.
     * <p>
     * 64 is chosen to amortize the cost of writability checks while still
     * being responsive to TCP back-pressure — similar to Netty's default
     * {@code ChannelConfig#getWriteSpinCount()}.
     */
    private static final int BACKLOG_DRAIN_BATCH_SIZE = 64;

    /**
     * CM-F1: Write Backlog to the {@link Channel} with backpressure awareness.
     * <p>
     * Instead of flushing all pending messages in a tight loop (which can overwhelm
     * the outbound buffer when thousands of messages accumulated during connection setup),
     * this drains in batches of {@link #BACKLOG_DRAIN_BATCH_SIZE}. After each batch, it
     * checks {@link Channel#isWritable()}. If the channel's write buffer has exceeded its
     * high-water mark, draining pauses and a {@code channelWritabilityChanged} callback
     * resumes it once the buffer drains below the low-water mark.
     */
    protected void writeBacklog() {
        final ConcurrentLinkedQueue<Object> queue = backlogQueue;
        backlogQueue = null;
        backlogSize.set(0);

        if (queue == null || queue.isEmpty()) {
            return;
        }

        drainBatch(queue);
    }

    /**
     * Drain up to {@link #BACKLOG_DRAIN_BATCH_SIZE} messages, then check writability.
     * If the channel is no longer writable, install a one-shot handler to resume
     * once the outbound buffer drains below the low-water mark.
     */
    private void drainBatch(ConcurrentLinkedQueue<Object> queue) {
        int written = 0;
        Object msg;
        while ((msg = queue.poll()) != null) {
            channel.write(msg).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            written++;

            if (written >= BACKLOG_DRAIN_BATCH_SIZE) {
                channel.flush();

                if (!channel.isWritable() && !queue.isEmpty()) {
                    // Outbound buffer is full — pause and wait for writability.
                    // Install a one-shot handler that resumes draining when
                    // channelWritabilityChanged fires (i.e., the buffer drops
                    // below the low-water mark).
                    channel.pipeline().addLast(new BacklogDrainHandler(queue));
                    return;
                }
                written = 0;
            }
        }

        // Final flush for the trailing partial batch.
        if (written > 0) {
            channel.flush();
        }
    }

    /**
     * One-shot handler that resumes backlog draining when the channel becomes
     * writable again. Removes itself from the pipeline after firing to avoid
     * leaking handlers.
     */
    private final class BacklogDrainHandler extends ChannelInboundHandlerAdapter {
        private final ConcurrentLinkedQueue<Object> queue;

        BacklogDrainHandler(ConcurrentLinkedQueue<Object> queue) {
            this.queue = queue;
        }

        @Override
        public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
            if (ctx.channel().isWritable()) {
                ctx.pipeline().remove(this);
                drainBatch(queue);
            }
            ctx.fireChannelWritabilityChanged();
        }

        @Override
        public void channelInactive(ChannelHandlerContext ctx) throws Exception {
            // Channel closed before we finished draining — release remaining messages.
            Object msg;
            while ((msg = queue.poll()) != null) {
                ReferenceCountedUtil.silentRelease(msg);
            }
            ctx.pipeline().remove(this);
            ctx.fireChannelInactive();
        }
    }

    /**
     * Clear the Backlog and release all objects.
     */
    protected void clearBacklog() {
        backlogQueue.forEach(ReferenceCountedUtil::silentRelease);
        backlogQueue = null;
        backlogSize.set(0);
    }

    /**
     * Write and Flush data
     *
     * @param o Data to be written
     * @throws IllegalStateException if the backlog queue has reached its maximum capacity
     */
    @NonNull
    public void writeAndFlush(Object o) {
        if (state == State.INITIALIZED) {
            ConcurrentLinkedQueue<Object> queue = backlogQueue;
            if (queue != null) {
                // Increment first, then check — this is atomic and prevents concurrent
                // threads from both passing the size check and exceeding the limit.
                // MEM-02: Use effectiveBacklogLimit() which reduces the limit under
                // memory pressure to shed load earlier and prevent OOM.
                int currentSize = backlogSize.incrementAndGet();
                int limit = effectiveBacklogLimit();
                if (currentSize > limit) {
                    backlogSize.decrementAndGet();
                    ReferenceCountedUtil.silentRelease(o);
                    // BUG-BACKLOG-CRASH: Return gracefully instead of throwing ISE.
                    // Callers (HttpConnection) should catch this and return 503.
                    throw new BacklogOverflowException("Backlog queue capacity exceeded: " + limit);
                }
                queue.add(o);
            } else {
                ReferenceCountedUtil.silentRelease(o);
            }
        } else if (state == State.CONNECTED_AND_ACTIVE && channel != null) {
            channel.writeAndFlush(o).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
        } else {
            ReferenceCountedUtil.silentRelease(o);
        }
    }

    /**
     * Write data without flushing. Use {@link #flush()} to flush buffered writes.
     * HIGH-10: Supports flush coalescing -- multiple writes batched before a single flush.
     *
     * @param o Data to be written
     */
    @NonNull
    public void write(Object o) {
        if (state == State.INITIALIZED) {
            ConcurrentLinkedQueue<Object> queue = backlogQueue;
            if (queue != null) {
                int currentSize = backlogSize.incrementAndGet();
                int limit = effectiveBacklogLimit();
                if (currentSize > limit) {
                    backlogSize.decrementAndGet();
                    ReferenceCountedUtil.silentRelease(o);
                    throw new BacklogOverflowException("Backlog queue capacity exceeded: " + limit);
                }
                queue.add(o);
            } else {
                ReferenceCountedUtil.silentRelease(o);
            }
        } else if (state == State.CONNECTED_AND_ACTIVE && channel != null) {
            channel.write(o);
        } else {
            ReferenceCountedUtil.silentRelease(o);
        }
    }

    /**
     * Flush all buffered writes to the underlying {@link Channel}.
     * HIGH-10: Called from channelReadComplete() to coalesce flushes.
     */
    public void flush() {
        if (state == State.CONNECTED_AND_ACTIVE && channel != null) {
            channel.flush();
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
     * Returns {@code true} if this connection uses the HTTP/2 protocol.
     * <p>
     * Subclasses that support HTTP/2 (e.g., HttpConnection) must override this
     * method to return the actual negotiated protocol status. The default
     * implementation returns {@code false}.
     *
     * @return {@code true} if this is an HTTP/2 connection, {@code false} otherwise
     */
    public boolean isHttp2() {
        return false;
    }

    /**
     * Send a GOAWAY frame to signal graceful shutdown per RFC 9113 Section 6.8.
     * <p>
     * The default implementation is a no-op that returns {@code null}. Subclasses
     * that support HTTP/2 (e.g., HttpConnection) must override this to write
     * a GOAWAY frame with NO_ERROR to the channel.
     *
     * @return {@link ChannelFuture} for the write, or {@code null} if GOAWAY is
     *         not applicable for this connection type.
     */
    public ChannelFuture sendGoaway() {
        return null;
    }

    /**
     * MEM-02: Set the memory pressure supplier. When this supplier returns {@code true},
     * the effective backlog limit is reduced to shed load earlier during memory pressure.
     *
     * @param supplier a fast, non-blocking supplier that returns {@code true} when memory
     *                 is pressured, or {@code null} to disable memory-aware shedding
     */
    public void setMemoryPressureSupplier(BooleanSupplier supplier) {
        this.memoryPressureSupplier = supplier;
    }

    /**
     * MEM-02: Returns the effective backlog limit based on current memory pressure.
     * Under normal conditions, returns {@link #maxBacklogSize}. Under memory pressure,
     * returns {@code maxBacklogSize / 4} to shed load earlier.
     */
    protected int effectiveBacklogLimit() {
        BooleanSupplier supplier = memoryPressureSupplier;
        if (supplier != null && supplier.getAsBoolean()) {
            return Math.max(1, maxBacklogSize / MEMORY_PRESSURE_BACKLOG_RATIO_DIVISOR);
        }
        return maxBacklogSize;
    }

    /**
     * MEM-02: Estimate the memory usage of the current backlog queue in bytes.
     * Uses a conservative estimate of 256 bytes per queued message (covers typical
     * HTTP request/response objects including headers, small bodies, and object overhead).
     * Useful for memory budget admission decisions.
     *
     * @return estimated backlog memory usage in bytes, or 0 if backlog is empty/null
     */
    public long estimateBacklogMemoryBytes() {
        int count = backlogSize.get();
        if (count <= 0) {
            return 0;
        }
        // 256 bytes per message is conservative: an HttpRequest with headers is typically
        // 200-500 bytes on heap, plus the ConcurrentLinkedQueue node (~32 bytes).
        return count * 256L;
    }

    /**
     * Close this {@link Connection}
     */
    public synchronized void close() {
        // If Backlog Queue contains something, then clear it before closing the connection.
        if (backlogQueue != null && !backlogQueue.isEmpty()) {
            clearBacklog();
        }

        // Remove this connection from Node
        node.removeConnection(this);

        // If Channel is not null, then close it.
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
