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

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link ReconnectStormProtector}.
 *
 * <p>Validates the token bucket rate-limiting behavior for controlling reconnection
 * storms during control plane failover. Tests burst capacity, token exhaustion,
 * refill behavior, and estimated wait time calculation.</p>
 */
class ReconnectStormProtectorTest {

    @Test
    @Timeout(10)
    void testBurstAllowsMaxBurstAdmits() {
        int maxBurst = 5;
        int refillRate = 1;
        ReconnectStormProtector protector = new ReconnectStormProtector(maxBurst, refillRate);

        // Should admit exactly maxBurst nodes
        for (int i = 0; i < maxBurst; i++) {
            assertTrue(protector.tryAdmit(),
                    "Admit #" + (i + 1) + " should succeed within burst capacity");
        }

        // Next admit should be rejected (all burst tokens consumed)
        assertFalse(protector.tryAdmit(),
                "Admit after burst exhaustion should be rejected");
    }

    @Test
    @Timeout(10)
    void testRefillReplenishesTokens() throws InterruptedException {
        int maxBurst = 2;
        int refillRate = 10; // 10 tokens/second -> ~100ms per token
        ReconnectStormProtector protector = new ReconnectStormProtector(maxBurst, refillRate);

        // Exhaust all tokens
        for (int i = 0; i < maxBurst; i++) {
            assertTrue(protector.tryAdmit());
        }
        assertFalse(protector.tryAdmit(), "Should be exhausted");

        // Wait for refill (1 second should give us up to refillRate tokens)
        Thread.sleep(1100);

        // After refill, at least one token should be available
        assertTrue(protector.tryAdmit(),
                "Should be able to admit after refill period");
    }

    @Test
    @Timeout(10)
    void testEstimatedWaitSecondsWhenTokensAvailable() {
        ReconnectStormProtector protector = new ReconnectStormProtector(10, 5);

        // Tokens are available, so wait should be 0
        assertEquals(0, protector.estimatedWaitSeconds(),
                "Wait should be 0 when tokens are available");
    }

    @Test
    @Timeout(10)
    void testEstimatedWaitSecondsWhenExhausted() {
        ReconnectStormProtector protector = new ReconnectStormProtector(1, 1);

        // Exhaust the single token
        assertTrue(protector.tryAdmit());
        assertFalse(protector.tryAdmit());

        long wait = protector.estimatedWaitSeconds();
        assertTrue(wait >= 1, "Wait should be >= 1 second when exhausted, got: " + wait);
    }

    @Test
    void testInvalidMaxBurstThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStormProtector(0, 1),
                "maxBurst=0 should be rejected");

        assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStormProtector(-1, 1),
                "maxBurst=-1 should be rejected");
    }

    @Test
    void testInvalidRefillRateThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStormProtector(1, 0),
                "refillRate=0 should be rejected");

        assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStormProtector(1, -1),
                "refillRate=-1 should be rejected");
    }
}
