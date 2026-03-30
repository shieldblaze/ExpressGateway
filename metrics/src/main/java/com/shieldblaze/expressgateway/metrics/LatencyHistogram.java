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

import java.util.Arrays;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;

/**
 * Lock-free latency histogram with percentile queries.
 *
 * <p>Uses a circular buffer of recent samples for percentile computation
 * and LongAdder-based counters for aggregate statistics. This approach
 * avoids the memory overhead of a full HDR Histogram while providing
 * accurate percentiles over recent samples.</p>
 *
 * <p>The buffer size is configurable (must be power of 2). Default is 4096
 * entries, giving accurate percentiles over approximately the last 4096
 * latency recordings.</p>
 *
 * <p>Thread safety: Recording is lock-free via CAS on the write index.
 * Percentile queries take an O(n log n) snapshot -- suitable for periodic
 * scraping, not per-request.</p>
 */
public final class LatencyHistogram {

    private final int bufferSize;
    private final int bufferMask;
    private final long[] buffer;
    private final AtomicLong writeIndex = new AtomicLong();

    private final LongAdder totalCount = new LongAdder();
    private final LongAdder totalSum = new LongAdder();
    private final AtomicLong minValue = new AtomicLong(Long.MAX_VALUE);
    private final AtomicLong maxValue = new AtomicLong(Long.MIN_VALUE);

    /**
     * Create a histogram with default buffer size (4096 samples).
     */
    public LatencyHistogram() {
        this(4096);
    }

    /**
     * Create a histogram with the specified buffer size.
     *
     * @param bufferSize must be a power of 2
     */
    public LatencyHistogram(int bufferSize) {
        if (bufferSize <= 0 || (bufferSize & (bufferSize - 1)) != 0) {
            throw new IllegalArgumentException("bufferSize must be a positive power of 2, was: " + bufferSize);
        }
        this.bufferSize = bufferSize;
        this.bufferMask = bufferSize - 1;
        this.buffer = new long[bufferSize];
    }

    /**
     * Record a latency value (lock-free).
     *
     * @param latencyMs latency in milliseconds (must be >= 0)
     */
    public void record(long latencyMs) {
        long idx = writeIndex.getAndIncrement();
        buffer[(int) (idx & bufferMask)] = latencyMs;

        totalCount.increment();
        totalSum.add(latencyMs);

        // Update min atomically
        long currentMin;
        do {
            currentMin = minValue.get();
            if (latencyMs >= currentMin) break;
        } while (!minValue.compareAndSet(currentMin, latencyMs));

        // Update max atomically
        long currentMax;
        do {
            currentMax = maxValue.get();
            if (latencyMs <= currentMax) break;
        } while (!maxValue.compareAndSet(currentMax, latencyMs));
    }

    /**
     * Returns total number of recorded samples.
     */
    public long count() {
        return totalCount.sum();
    }

    /**
     * Returns sum of all recorded samples.
     */
    public long sum() {
        return totalSum.sum();
    }

    /**
     * Returns minimum recorded value, or 0 if no samples.
     */
    public long min() {
        long val = minValue.get();
        return val == Long.MAX_VALUE ? 0 : val;
    }

    /**
     * Returns maximum recorded value, or 0 if no samples.
     */
    public long max() {
        long val = maxValue.get();
        return val == Long.MIN_VALUE ? 0 : val;
    }

    /**
     * Returns 50th percentile of recent samples.
     */
    public long p50() {
        return percentile(50);
    }

    /**
     * Returns 95th percentile of recent samples.
     */
    public long p95() {
        return percentile(95);
    }

    /**
     * Returns 99th percentile of recent samples.
     */
    public long p99() {
        return percentile(99);
    }

    /**
     * Returns 99.9th percentile of recent samples.
     */
    public long p999() {
        return percentile(99.9);
    }

    /**
     * Compute an arbitrary percentile from the circular buffer.
     *
     * @param percentile the percentile to compute (0-100)
     * @return the value at the requested percentile, or 0 if no samples
     */
    public long percentile(double percentile) {
        long total = totalCount.sum();
        if (total == 0) {
            return 0;
        }

        int sampleCount = (int) Math.min(total, bufferSize);
        long[] snapshot = new long[sampleCount];
        long currentIdx = writeIndex.get();
        for (int i = 0; i < sampleCount; i++) {
            snapshot[i] = buffer[(int) ((currentIdx - sampleCount + i) & bufferMask)];
        }

        Arrays.sort(snapshot);
        int rank = (int) Math.ceil(percentile / 100.0 * sampleCount) - 1;
        return snapshot[Math.max(0, Math.min(rank, sampleCount - 1))];
    }

    /**
     * Returns the average of all recorded samples, or 0 if no samples.
     */
    public long average() {
        long c = totalCount.sum();
        return c == 0 ? 0 : totalSum.sum() / c;
    }
}
