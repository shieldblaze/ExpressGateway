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
package com.shieldblaze.expressgateway.controlplane.cluster;

import io.github.bucket4j.Bandwidth;
import io.github.bucket4j.Bucket;
import io.github.bucket4j.Refill;

import java.time.Duration;

/**
 * Rate-limits data-plane node reconnections to prevent thundering herd on CP failover.
 *
 * <p>When a control plane instance fails, all data-plane nodes connected to it will
 * attempt to reconnect to other CP instances simultaneously. Without rate limiting,
 * this reconnect storm can overwhelm the surviving instances and cascade the failure.</p>
 *
 * <h3>Algorithm: Token Bucket</h3>
 * <ul>
 *   <li>Initial burst capacity of {@code maxBurst} tokens allows a burst of reconnections</li>
 *   <li>Tokens are refilled at {@code refillRatePerSecond} tokens per second</li>
 *   <li>Each reconnection attempt consumes one token</li>
 *   <li>If no tokens are available, the reconnection is rejected; the caller should
 *       respond with {@code RESOURCE_EXHAUSTED} and a {@code retry-after} hint
 *       based on {@link #estimatedWaitSeconds()}</li>
 * </ul>
 *
 * <p>Thread safety: {@link Bucket} from bucket4j is thread-safe and lock-free.</p>
 */
public final class ReconnectStormProtector {

    private final Bucket bucket;
    private final int refillRatePerSecond;

    /**
     * Creates a new reconnect storm protector.
     *
     * @param maxBurst             the maximum number of nodes that can reconnect immediately (burst capacity)
     * @param refillRatePerSecond  the rate at which reconnection tokens are replenished per second
     * @throws IllegalArgumentException if either parameter is not positive
     */
    public ReconnectStormProtector(int maxBurst, int refillRatePerSecond) {
        if (maxBurst < 1) {
            throw new IllegalArgumentException("maxBurst must be >= 1, got: " + maxBurst);
        }
        if (refillRatePerSecond < 1) {
            throw new IllegalArgumentException("refillRatePerSecond must be >= 1, got: " + refillRatePerSecond);
        }

        this.refillRatePerSecond = refillRatePerSecond;

        Bandwidth limit = Bandwidth.classic(maxBurst,
                Refill.greedy(refillRatePerSecond, Duration.ofSeconds(1)));

        this.bucket = Bucket.builder()
                .addLimit(limit)
                .withNanosecondPrecision()
                .build();
    }

    /**
     * Attempts to admit a reconnecting data-plane node.
     *
     * <p>Consumes one token from the bucket. If no tokens are available, the node
     * should be rejected with a {@code RESOURCE_EXHAUSTED} gRPC status and a
     * retry-after hint from {@link #estimatedWaitSeconds()}.</p>
     *
     * @return {@code true} if the node is admitted, {@code false} if rate-limited
     */
    public boolean tryAdmit() {
        return bucket.tryConsume(1);
    }

    /**
     * Returns the estimated number of seconds until the next token becomes available.
     *
     * <p>This value is suitable for a {@code retry-after} header or gRPC metadata hint.
     * Returns 0 if tokens are currently available. The estimate is computed from the
     * configured refill rate and is approximate -- actual availability depends on
     * concurrent consumers.</p>
     *
     * @return estimated seconds to wait, or 0 if tokens are available
     */
    public long estimatedWaitSeconds() {
        long availableTokens = bucket.getAvailableTokens();
        if (availableTokens > 0) {
            return 0;
        }
        // At minimum, one second for the next refill tick. In practice, with greedy
        // refill, the wait is ceil(1 / refillRatePerSecond) seconds for a single token.
        // For high refill rates (>= 1/s), the wait is always 1 second or less.
        return Math.max(1, 1 + (-availableTokens) / refillRatePerSecond);
    }
}
