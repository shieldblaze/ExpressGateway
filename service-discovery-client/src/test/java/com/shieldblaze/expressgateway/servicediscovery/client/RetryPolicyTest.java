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

import org.junit.jupiter.api.Test;

import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class RetryPolicyTest {

    @Test
    void defaultPolicy() {
        RetryPolicy policy = RetryPolicy.DEFAULT;
        assertEquals(3, policy.maxAttempts());
        assertEquals(Duration.ofMillis(100), policy.baseDelay());
        assertEquals(Duration.ofSeconds(5), policy.maxDelay());
    }

    @Test
    void nonePolicy() {
        RetryPolicy policy = RetryPolicy.NONE;
        assertEquals(1, policy.maxAttempts());
    }

    @Test
    void firstAttemptHasNoDelay() {
        RetryPolicy policy = RetryPolicy.DEFAULT;
        assertEquals(0, policy.delayMillis(0));
    }

    @Test
    void exponentialBackoff() {
        RetryPolicy policy = new RetryPolicy(5, Duration.ofMillis(100), Duration.ofSeconds(10), Duration.ZERO);
        assertEquals(0, policy.delayMillis(0));
        assertEquals(200, policy.delayMillis(1));  // 100 * 2^1
        assertEquals(400, policy.delayMillis(2));  // 100 * 2^2
        assertEquals(800, policy.delayMillis(3));  // 100 * 2^3
    }

    @Test
    void cappedAtMaxDelay() {
        RetryPolicy policy = new RetryPolicy(10, Duration.ofMillis(100), Duration.ofMillis(500), Duration.ZERO);
        // After enough attempts, delay is capped
        long delay = policy.delayMillis(5); // 100 * 2^5 = 3200, but capped at 500
        assertEquals(500, delay);
    }

    @Test
    void jitterAddsRandomness() {
        RetryPolicy policy = new RetryPolicy(3, Duration.ofMillis(100), Duration.ofSeconds(5), Duration.ofMillis(100));
        long delay = policy.delayMillis(1);
        // Should be 200 (exponential) + [0,100) (jitter) = [200, 300)
        assertTrue(delay >= 200 && delay < 300, "Delay should be in [200, 300), got: " + delay);
    }

    @Test
    void invalidMaxAttemptsThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> new RetryPolicy(0, Duration.ofMillis(100), Duration.ofSeconds(5), Duration.ZERO));
    }
}
