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
package com.shieldblaze.expressgateway.protocol.quic;

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFuture;
import io.netty.handler.codec.quic.QuicChannel;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.atomic.AtomicInteger;

/**
 * Represents a QUIC backend connection wrapping a {@link QuicChannel}.
 *
 * <p>Unlike TCP-based connections, a QUIC connection carries multiple independent
 * bidirectional streams (RFC 9000 Section 2). Each request-response exchange
 * maps to one QUIC stream, with no head-of-line blocking between streams. This class
 * tracks the number of active streams for pool acquisition decisions.</p>
 *
 * <h3>Stream Counting</h3>
 * The active stream count is incremented when a new request stream is opened
 * ({@link #incrementActiveStreams()}) and decremented when the stream completes
 * ({@link #decrementActiveStreams()}). The pool uses {@link #hasStreamCapacity(int)}
 * to determine whether this connection can accept additional requests.
 *
 * <h3>Thread Safety</h3>
 * Accessed from the frontend EventLoop (acquire/increment), the backend QUIC EventLoop
 * (decrement on response completion), and the pool scan (read during acquire).
 * {@link AtomicInteger} provides cross-thread safety for the stream counter.
 *
 * <h3>Idle Tracking</h3>
 * When the active stream count drops to zero, {@link #idleSinceNanos} is set for
 * pool eviction decisions. QUIC connections remain in the pool and are evicted only
 * when idle for too long or when the QUIC connection closes.
 */
public class QuicConnection extends Connection {

    private static final Logger logger = LogManager.getLogger(QuicConnection.class);

    /**
     * The underlying QUIC connection channel. Set after the QUIC handshake completes.
     * Must be volatile because it is written from the QUIC EventLoop (handshake callback)
     * and read from frontend EventLoop threads (pool acquisition, stream creation).
     */
    private volatile QuicChannel quicChannel;

    /**
     * Number of active QUIC streams currently using this backend connection.
     * Used by {@link QuicConnectionPool} to determine if this connection has capacity
     * for additional streams ({@code activeStreams < maxConcurrentStreams}).
     */
    private final AtomicInteger activeStreamCount = new AtomicInteger(0);

    /**
     * Timestamp (via {@link System#nanoTime()}) when this connection last became
     * fully idle (zero active streams). Used by the eviction sweep to determine if
     * the connection has exceeded the idle timeout.
     */
    private volatile long idleSinceNanos;

    public QuicConnection(Node node) {
        super(node);
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        if (channelFuture.isSuccess()) {
            writeBacklog();
        } else {
            clearBacklog();
        }
    }

    /**
     * Set the underlying {@link QuicChannel} after the QUIC handshake completes.
     *
     * @param quicChannel the connected QUIC channel
     */
    public void quicChannel(QuicChannel quicChannel) {
        this.quicChannel = quicChannel;
    }

    /**
     * Returns the underlying {@link QuicChannel}, or {@code null} if the QUIC
     * handshake has not yet completed.
     */
    public QuicChannel quicChannel() {
        return quicChannel;
    }

    /**
     * Increment the active stream count when a new QUIC stream is opened.
     * Clears the idle timestamp since the connection is now actively serving traffic.
     *
     * @return the new active stream count after incrementing
     */
    public int incrementActiveStreams() {
        int newCount = activeStreamCount.incrementAndGet();
        // Clear idle timestamp AFTER increment so a concurrent decrement-to-zero
        // cannot race to set idleSinceNanos while we still have active streams.
        idleSinceNanos = 0;
        return newCount;
    }

    /**
     * Decrement the active stream count when a QUIC stream completes.
     * If the count reaches zero, the idle timestamp is set for eviction tracking.
     *
     * @return the new active stream count after decrementing
     */
    public int decrementActiveStreams() {
        int newCount = activeStreamCount.decrementAndGet();
        if (newCount == 0) {
            idleSinceNanos = System.nanoTime();
        }
        return newCount;
    }

    /**
     * Returns the current number of active QUIC streams on this connection.
     */
    public int activeStreams() {
        return activeStreamCount.get();
    }

    /**
     * Returns {@code true} if this connection can accept at least one more stream.
     *
     * @param maxConcurrentStreams the maximum streams allowed per connection
     */
    public boolean hasStreamCapacity(int maxConcurrentStreams) {
        return activeStreamCount.get() < maxConcurrentStreams;
    }

    /**
     * Returns the {@link System#nanoTime()} timestamp when this connection last became
     * idle (zero active streams), or 0 if the connection is currently active.
     */
    public long idleSinceNanos() {
        return idleSinceNanos;
    }

    /**
     * Send a QUIC GOAWAY / graceful close to the peer.
     *
     * @return {@link ChannelFuture} for the write, or {@code null} if not possible
     */
    @Override
    public ChannelFuture sendGoaway() {
        QuicChannel qc = quicChannel;
        if (qc == null || !qc.isActive()) {
            return null;
        }
        return qc.close(true, 0x0, Unpooled.EMPTY_BUFFER);
    }

    /**
     * Close this QUIC connection. Closes the underlying QuicChannel which
     * will abort all active streams and release QUIC transport resources.
     */
    @Override
    public synchronized void close() {
        if (backlogQueue != null && !backlogQueue.isEmpty()) {
            clearBacklog();
        }

        node().removeConnection(this);

        QuicChannel qc = quicChannel;
        if (qc != null && qc.isActive()) {
            qc.close();
        }

        if (channel != null) {
            channel.close();
        }
    }

    @Override
    public String toString() {
        return "QuicConnection{" + "activeStreams=" + activeStreamCount.get()
                + ", idleSinceNanos=" + idleSinceNanos
                + ", Connection=" + super.toString() + '}';
    }
}
