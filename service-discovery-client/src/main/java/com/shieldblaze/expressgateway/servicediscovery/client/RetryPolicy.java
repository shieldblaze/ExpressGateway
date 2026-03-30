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
package com.shieldblaze.expressgateway.servicediscovery.client;

import java.time.Duration;
import java.util.concurrent.ThreadLocalRandom;

/**
 * Retry policy with exponential backoff and jitter for service discovery calls.
 * The jitter prevents thundering-herd retries when multiple clients fail simultaneously.
 *
 * <p>Delay formula: {@code min(maxDelay, baseDelay * 2^attempt) + random(0, jitterBound)}</p>
 *
 * @param maxAttempts maximum number of retry attempts (including the initial call)
 * @param baseDelay   initial backoff delay
 * @param maxDelay    maximum backoff delay cap
 * @param jitterBound maximum random jitter added to each delay
 */
public record RetryPolicy(
        int maxAttempts,
        Duration baseDelay,
        Duration maxDelay,
        Duration jitterBound
) {

    /**
     * Default retry policy: 3 attempts, 100ms base, 5s max, 50ms jitter.
     */
    public static final RetryPolicy DEFAULT = new RetryPolicy(
            3, Duration.ofMillis(100), Duration.ofSeconds(5), Duration.ofMillis(50));

    /**
     * No-retry policy: execute once, fail immediately on error.
     */
    public static final RetryPolicy NONE = new RetryPolicy(
            1, Duration.ZERO, Duration.ZERO, Duration.ZERO);

    public RetryPolicy {
        if (maxAttempts < 1) {
            throw new IllegalArgumentException("maxAttempts must be >= 1");
        }
    }

    /**
     * Calculate the delay in milliseconds for the given attempt (0-indexed).
     */
    public long delayMillis(int attempt) {
        if (attempt <= 0) {
            return 0;
        }
        long exponential = baseDelay.toMillis() * (1L << Math.min(attempt, 30));
        long capped = Math.min(exponential, maxDelay.toMillis());
        long jitter = jitterBound.toMillis() > 0
                ? ThreadLocalRandom.current().nextLong(jitterBound.toMillis())
                : 0;
        return capped + jitter;
    }
}
