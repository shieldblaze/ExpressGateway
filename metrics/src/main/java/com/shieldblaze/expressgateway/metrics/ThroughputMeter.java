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
package com.shieldblaze.expressgateway.metrics;

import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;

/**
 * Lock-free throughput meter using a sliding window.
 *
 * <p>Tracks both bytes/sec and requests/sec using a simple two-phase
 * sliding window. The window consists of two buckets: the current
 * active bucket and the previous bucket. When queried, the rate is
 * computed by combining both buckets, weighted by how far into the
 * current interval we are.</p>
 *
 * <p>This is the same EWMA-free approach used by Netty's traffic shaping
 * handlers: simple, deterministic, and lock-free.</p>
 *
 * <p>Thread safety: All fields use atomic operations. No locks.</p>
 */
public final class ThroughputMeter {

    private static final long DEFAULT_INTERVAL_NANOS = 1_000_000_000L; // 1 second

    private final long intervalNanos;

    // Total counters (monotonic)
    private final LongAdder totalCount = new LongAdder();
    private final LongAdder totalBytes = new LongAdder();

    // Sliding window buckets
    private final AtomicLong currentWindowStart = new AtomicLong(System.nanoTime());
    private final LongAdder currentWindowCount = new LongAdder();
    private final LongAdder currentWindowBytes = new LongAdder();
    private final AtomicLong previousWindowCount = new AtomicLong();
    private final AtomicLong previousWindowBytes = new AtomicLong();

    /**
     * Create a throughput meter with the default 1-second interval.
     */
    public ThroughputMeter() {
        this(DEFAULT_INTERVAL_NANOS);
    }

    /**
     * Create a throughput meter with a custom interval.
     *
     * @param intervalNanos interval in nanoseconds (must be > 0)
     */
    public ThroughputMeter(long intervalNanos) {
        if (intervalNanos <= 0) {
            throw new IllegalArgumentException("intervalNanos must be > 0, was: " + intervalNanos);
        }
        this.intervalNanos = intervalNanos;
    }

    /**
     * Record a single event (request).
     */
    public void mark() {
        mark(1, 0);
    }

    /**
     * Record a single event with a byte count.
     *
     * @param bytes number of bytes for this event
     */
    public void markBytes(long bytes) {
        mark(1, bytes);
    }

    /**
     * Record multiple events.
     *
     * @param count number of events
     * @param bytes total bytes for these events
     */
    public void mark(long count, long bytes) {
        rollWindow();
        totalCount.add(count);
        totalBytes.add(bytes);
        currentWindowCount.add(count);
        currentWindowBytes.add(bytes);
    }

    /**
     * Returns the current request rate per second.
     */
    public double rate() {
        rollWindow();
        long now = System.nanoTime();
        long windowStart = currentWindowStart.get();
        long elapsed = now - windowStart;

        if (elapsed <= 0) elapsed = 1;

        double weight = Math.min(1.0, (double) elapsed / intervalNanos);
        long currentCount = currentWindowCount.sum();
        long prevCount = previousWindowCount.get();

        // Weighted combination: fraction of current window + remainder from previous
        double interpolated = currentCount + prevCount * (1.0 - weight);
        return interpolated / (intervalNanos / 1_000_000_000.0);
    }

    /**
     * Returns the current byte rate per second.
     */
    public double byteRate() {
        rollWindow();
        long now = System.nanoTime();
        long windowStart = currentWindowStart.get();
        long elapsed = now - windowStart;

        if (elapsed <= 0) elapsed = 1;

        double weight = Math.min(1.0, (double) elapsed / intervalNanos);
        long currentBytes = currentWindowBytes.sum();
        long prevBytes = previousWindowBytes.get();

        double interpolated = currentBytes + prevBytes * (1.0 - weight);
        return interpolated / (intervalNanos / 1_000_000_000.0);
    }

    /**
     * Returns total event count since creation.
     */
    public long totalCount() {
        return totalCount.sum();
    }

    /**
     * Returns total bytes since creation.
     */
    public long totalBytes() {
        return totalBytes.sum();
    }

    /**
     * Roll the window if the current interval has elapsed.
     * Uses CAS to ensure only one thread performs the roll.
     */
    private void rollWindow() {
        long now = System.nanoTime();
        long windowStart = currentWindowStart.get();
        if (now - windowStart >= intervalNanos) {
            if (currentWindowStart.compareAndSet(windowStart, now)) {
                previousWindowCount.set(currentWindowCount.sumThenReset());
                previousWindowBytes.set(currentWindowBytes.sumThenReset());
            }
        }
    }
}
