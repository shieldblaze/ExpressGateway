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
package com.shieldblaze.expressgateway.security;

import com.google.common.cache.Cache;
import com.google.common.cache.CacheBuilder;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Objects;
import java.util.concurrent.atomic.LongAdder;

/**
 * Per-IP connection rate limiter using a sliding window counter.
 *
 * <p>This handler enforces a maximum number of new connections per source IP
 * within a configurable time window. It uses a sliding window algorithm where
 * each IP address tracks connection timestamps in a bounded circular buffer.</p>
 *
 * <p>The implementation uses a Guava {@link Cache} with LRU eviction to bound
 * memory under DDoS conditions with millions of unique source IPs.</p>
 *
 * <p>Integration: Add this handler to the Netty pipeline before the main
 * application handlers. It is {@link ChannelHandler.Sharable} and can be
 * added to multiple channels.</p>
 */
@ChannelHandler.Sharable
public final class ConnectionRateLimiter extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(ConnectionRateLimiter.class);
    private static final int DEFAULT_MAX_TRACKED_IPS = 100_000;

    private final int maxConnectionsPerWindow;
    private final long windowNanos;
    private final Cache<String, SlidingWindowCounter> perIpCounters;
    private final LongAdder totalAccepted = new LongAdder();
    private final LongAdder totalRejected = new LongAdder();

    /**
     * @param maxConnectionsPerWindow Maximum new connections allowed per IP within the window
     * @param window                  Time window duration
     */
    public ConnectionRateLimiter(int maxConnectionsPerWindow, Duration window) {
        this(maxConnectionsPerWindow, window, DEFAULT_MAX_TRACKED_IPS);
    }

    /**
     * @param maxConnectionsPerWindow Maximum new connections allowed per IP within the window
     * @param window                  Time window duration
     * @param maxTrackedIps           Maximum IPs to track (LRU eviction beyond this)
     */
    public ConnectionRateLimiter(int maxConnectionsPerWindow, Duration window, int maxTrackedIps) {
        if (maxConnectionsPerWindow <= 0) {
            throw new IllegalArgumentException("maxConnectionsPerWindow must be > 0, was: " + maxConnectionsPerWindow);
        }
        Objects.requireNonNull(window, "window");
        if (window.isZero() || window.isNegative()) {
            throw new IllegalArgumentException("window must be positive, was: " + window);
        }

        this.maxConnectionsPerWindow = maxConnectionsPerWindow;
        this.windowNanos = window.toNanos();
        this.perIpCounters = CacheBuilder.newBuilder()
                .maximumSize(maxTrackedIps)
                .expireAfterAccess(window.multipliedBy(2))
                .build();
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        var remoteAddr = ctx.channel().remoteAddress();
        if (remoteAddr instanceof InetSocketAddress inet) {
            String key = inet.getAddress().getHostAddress();
            SlidingWindowCounter counter = perIpCounters.get(key,
                    () -> new SlidingWindowCounter(maxConnectionsPerWindow));

            long now = System.nanoTime();
            if (!counter.tryAcquire(now, windowNanos)) {
                logger.debug("Connection rate limit exceeded for {}", key);
                totalRejected.increment();
                ctx.close();
                return;
            }
        }

        totalAccepted.increment();
        super.channelActive(ctx);
    }

    /**
     * Returns total accepted connections.
     */
    public long totalAccepted() {
        return totalAccepted.sum();
    }

    /**
     * Returns total rejected connections.
     */
    public long totalRejected() {
        return totalRejected.sum();
    }

    /**
     * Returns the number of tracked IPs.
     */
    public long trackedIpCount() {
        return perIpCounters.size();
    }

    /**
     * Sliding window counter using a circular buffer of timestamps.
     * Each call to {@link #tryAcquire(long, long)} evicts expired timestamps
     * and checks if the current count is within the limit.
     *
     * <p>Thread safety: Synchronized on the counter instance. Since each IP gets
     * its own counter and connection setup is not the hot path (compared to
     * per-packet operations), this is acceptable overhead.</p>
     */
    static final class SlidingWindowCounter {
        private final long[] timestamps;
        private int head;
        private int count;

        SlidingWindowCounter(int maxEntries) {
            this.timestamps = new long[maxEntries];
            this.head = 0;
            this.count = 0;
        }

        synchronized boolean tryAcquire(long nowNanos, long windowNanos) {
            // Evict expired entries from the tail
            long cutoff = nowNanos - windowNanos;
            while (count > 0) {
                int tailIdx = (head - count + timestamps.length) % timestamps.length;
                if (timestamps[tailIdx] <= cutoff) {
                    count--;
                } else {
                    break;
                }
            }

            if (count >= timestamps.length) {
                return false;
            }

            timestamps[head] = nowNanos;
            head = (head + 1) % timestamps.length;
            count++;
            return true;
        }

        synchronized int currentCount() {
            return count;
        }
    }
}
