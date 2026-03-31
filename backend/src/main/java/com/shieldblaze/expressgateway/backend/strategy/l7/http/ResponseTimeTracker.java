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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

import com.shieldblaze.expressgateway.backend.Node;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * <p> Tracks response time per {@link Node} using Exponentially Weighted Moving Average (EWMA). </p>
 *
 * <p> EWMA formula: {@code ewma = alpha * sample + (1 - alpha) * ewma} </p>
 *
 * <p> This tracker is thread-safe. Response times are recorded using compare-and-swap on
 * AtomicLong, which stores the EWMA as a {@code long} representation of the double bits.
 * This avoids synchronization on the hot path while maintaining correctness under
 * concurrent updates. </p>
 *
 * <p> Cold start handling: The tracker reports a node as "cold" until it has accumulated
 * at least {@link #COLD_START_THRESHOLD} samples. During cold start, the load balancer
 * should use a fallback strategy (e.g., round-robin) instead of relying on the EWMA value. </p>
 */
public final class ResponseTimeTracker {

    /**
     * Minimum number of samples required before the EWMA value is considered reliable.
     */
    static final int COLD_START_THRESHOLD = 10;

    /**
     * Per-node EWMA state. Uses AtomicLong to store the EWMA double as raw bits
     * for lock-free CAS updates, and AtomicInteger for the sample count.
     */
    private static final class EwmaState {
        /**
         * The EWMA value stored as Double.doubleToLongBits. Initialized to 0.0.
         */
        final AtomicLong ewmaBits = new AtomicLong(Double.doubleToLongBits(0.0));

        /**
         * Number of samples recorded. Saturates at Integer.MAX_VALUE.
         */
        final AtomicInteger sampleCount = new AtomicInteger(0);
    }

    private final Map<Node, EwmaState> states = new ConcurrentHashMap<>();

    /**
     * The EWMA smoothing factor. Values closer to 1.0 give more weight to recent
     * observations; values closer to 0.0 give more weight to historical observations.
     * Valid range: (0.0, 1.0].
     */
    private final double alpha;

    /**
     * Create a {@link ResponseTimeTracker} with the specified alpha.
     *
     * @param alpha EWMA smoothing factor, must be in (0.0, 1.0]
     * @throws IllegalArgumentException if alpha is out of range
     */
    public ResponseTimeTracker(double alpha) {
        if (alpha <= 0.0 || alpha > 1.0) {
            throw new IllegalArgumentException("Alpha must be in (0.0, 1.0], got: " + alpha);
        }
        this.alpha = alpha;
    }

    /**
     * Create a {@link ResponseTimeTracker} with the default alpha of 0.5.
     */
    public ResponseTimeTracker() {
        this(0.5);
    }

    /**
     * Record a response time sample for the given node.
     *
     * <p> Uses a CAS loop to atomically update the EWMA. Under normal contention
     * (multiple EventLoop threads recording for the same node), this converges in
     * 1-2 CAS attempts. The CAS loop is lock-free and wait-free in practice. </p>
     *
     * @param node               the backend node
     * @param responseTimeMillis the response time in milliseconds, must be &gt;= 0
     */
    public void record(Node node, long responseTimeMillis) {
        if (responseTimeMillis < 0) {
            return; // Ignore negative values (clock skew, etc.)
        }

        EwmaState state = states.computeIfAbsent(node, n -> new EwmaState());
        int count = state.sampleCount.get();

        // CAS loop to update the EWMA atomically.
        long oldBits;
        long newBits;
        do {
            oldBits = state.ewmaBits.get();
            double oldEwma = Double.longBitsToDouble(oldBits);

            double newEwma;
            if (count == 0) {
                // First sample: initialize EWMA to the observed value.
                newEwma = responseTimeMillis;
            } else {
                // EWMA update: alpha * sample + (1 - alpha) * oldEwma
                newEwma = alpha * responseTimeMillis + (1.0 - alpha) * oldEwma;
            }
            newBits = Double.doubleToLongBits(newEwma);
        } while (!state.ewmaBits.compareAndSet(oldBits, newBits));

        // Increment sample count. Saturate at Integer.MAX_VALUE to avoid overflow.
        if (count < Integer.MAX_VALUE) {
            state.sampleCount.incrementAndGet();
        }
    }

    /**
     * Get the current EWMA response time for the given node.
     *
     * @param node the backend node
     * @return the EWMA response time in milliseconds, or {@code 0.0} if no samples
     */
    public double getEwma(Node node) {
        EwmaState state = states.get(node);
        if (state == null) {
            return 0.0;
        }
        return Double.longBitsToDouble(state.ewmaBits.get());
    }

    /**
     * Get the number of response time samples recorded for the given node.
     *
     * @param node the backend node
     * @return the sample count
     */
    public int getSampleCount(Node node) {
        EwmaState state = states.get(node);
        if (state == null) {
            return 0;
        }
        return state.sampleCount.get();
    }

    /**
     * Returns {@code true} if the node has accumulated enough samples to be
     * considered "warm" (past the cold start threshold).
     *
     * @param node the backend node
     * @return {@code true} if sample count &gt;= {@link #COLD_START_THRESHOLD}
     */
    public boolean isWarm(Node node) {
        return getSampleCount(node) >= COLD_START_THRESHOLD;
    }

    /**
     * Remove tracking state for the given node. Should be called when
     * a node is removed from the cluster.
     *
     * @param node the backend node to remove
     */
    public void remove(Node node) {
        states.remove(node);
    }

    /**
     * Clear all tracked state.
     */
    public void clear() {
        states.clear();
    }
}
