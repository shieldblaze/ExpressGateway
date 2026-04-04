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
package com.shieldblaze.expressgateway.controlplane.agent;

import java.util.concurrent.ThreadLocalRandom;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Exponential backoff with jitter for reconnection to Control Plane.
 * Base=1s, max=30s, full jitter. Thread-safe.
 *
 * <p>Full jitter uniformly distributes the delay in {@code [0, min(maxDelay, base * 2^attempt))},
 * which spreads reconnecting clients across time and avoids thundering-herd problems when many
 * data-plane nodes lose connectivity to the control plane simultaneously.</p>
 */
public final class ReconnectStrategy {

    private final long baseDelayMs;
    private final long maxDelayMs;
    private final AtomicInteger attempt = new AtomicInteger(0);

    public ReconnectStrategy() {
        this(1000, 30_000);
    }

    public ReconnectStrategy(long baseDelayMs, long maxDelayMs) {
        if (baseDelayMs <= 0) {
            throw new IllegalArgumentException("baseDelayMs must be > 0, got: " + baseDelayMs);
        }
        if (maxDelayMs <= 0) {
            throw new IllegalArgumentException("maxDelayMs must be > 0, got: " + maxDelayMs);
        }
        if (maxDelayMs < baseDelayMs) {
            throw new IllegalArgumentException("maxDelayMs must be >= baseDelayMs");
        }
        this.baseDelayMs = baseDelayMs;
        this.maxDelayMs = maxDelayMs;
    }

    /**
     * Get next delay in milliseconds.
     *
     * <p>Computes exponential backoff capped at {@code maxDelayMs}, then applies
     * full jitter: uniform random in {@code [0, exponentialDelay)}. The attempt
     * counter is capped at 20 to prevent overflow in the shift operation.</p>
     *
     * @return delay in milliseconds, always >= 0
     */
    public long nextDelay() {
        int currentAttempt = attempt.getAndIncrement();
        long exponentialDelay = Math.min(maxDelayMs, baseDelayMs * (1L << Math.min(currentAttempt, 20)));
        // Full jitter: uniform random [0, exponentialDelay)
        return ThreadLocalRandom.current().nextLong(Math.max(1, exponentialDelay));
    }

    /**
     * Reset after successful connection.
     */
    public void reset() {
        attempt.set(0);
    }
}
