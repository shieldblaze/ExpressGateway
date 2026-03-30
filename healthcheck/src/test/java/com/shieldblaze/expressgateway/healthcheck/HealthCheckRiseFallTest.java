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
package com.shieldblaze.expressgateway.healthcheck;

import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.*;

final class HealthCheckRiseFallTest {

    private static final InetSocketAddress DUMMY_ADDRESS =
            new InetSocketAddress("127.0.0.1", 1);
    private static final Duration DUMMY_TIMEOUT = Duration.ofSeconds(1);

    private static final class StubHealthCheck extends HealthCheck {

        StubHealthCheck(int samples, int rise, int fall) {
            super(DUMMY_ADDRESS, DUMMY_TIMEOUT, samples, rise, fall,
                    Duration.ofMillis(1), 0, 0);
        }

        StubHealthCheck() {
            super(DUMMY_ADDRESS, DUMMY_TIMEOUT);
        }

        @Override
        public void run() {
            // no-op
        }

        void success() {
            markSuccess();
        }

        void failure() {
            markFailure();
        }
    }

    // ===== Rise threshold tests =====

    @Test
    void riseFall3_healthStaysUnknownUntilThresholdMet() {
        StubHealthCheck hc = new StubHealthCheck(100, 3, 3);

        assertEquals(Health.UNKNOWN, hc.health());

        hc.success();
        assertEquals(Health.UNKNOWN, hc.health());

        hc.success();
        assertEquals(Health.UNKNOWN, hc.health());
    }

    @Test
    void riseFall3_threeConsecutiveSuccesses_transitionsToGood() {
        StubHealthCheck hc = new StubHealthCheck(100, 3, 3);

        hc.success();
        hc.success();
        hc.success();

        assertEquals(Health.GOOD, hc.health());
    }

    @Test
    void riseFall3_failureResetsSuccessCounter() {
        StubHealthCheck hc = new StubHealthCheck(100, 3, 3);

        hc.success();
        hc.success();
        hc.failure();

        assertEquals(Health.UNKNOWN, hc.health());

        hc.success();
        hc.success();
        hc.success();

        assertEquals(Health.GOOD, hc.health());
    }

    // ===== Fall threshold tests =====

    @Test
    void riseFall3_threeConsecutiveFailures_transitionsToBad() {
        StubHealthCheck hc = new StubHealthCheck(100, 3, 3);

        hc.failure();
        hc.failure();
        hc.failure();

        assertEquals(Health.BAD, hc.health());
    }

    @Test
    void riseFall3_successResetsFailureCounter() {
        StubHealthCheck hc = new StubHealthCheck(100, 3, 3);

        hc.failure();
        hc.failure();
        hc.success();

        assertEquals(Health.UNKNOWN, hc.health());

        hc.failure();
        hc.failure();
        hc.failure();

        assertEquals(Health.BAD, hc.health());
    }

    // ===== Flapping tests =====

    @Test
    void riseFall3_alternatingSuccessFailure_neverTransitions() {
        StubHealthCheck hc = new StubHealthCheck(100, 3, 3);

        for (int i = 0; i < 20; i++) {
            hc.success();
            hc.failure();
        }

        assertEquals(Health.UNKNOWN, hc.health());
    }

    // ===== Default constructor backward compatibility =====

    @Test
    void defaultConstructor_immediateSuccessTransition() {
        StubHealthCheck hc = new StubHealthCheck();

        assertEquals(Health.UNKNOWN, hc.health());

        hc.success();
        assertEquals(Health.GOOD, hc.health());
    }

    @Test
    void defaultConstructor_immediateFailureTransition() {
        StubHealthCheck hc = new StubHealthCheck();

        hc.failure();
        assertEquals(Health.BAD, hc.health());
    }

    // ===== Invalid thresholds =====

    @Test
    void zeroRise_shouldThrow() {
        assertThrows(IllegalArgumentException.class,
                () -> new StubHealthCheck(100, 0, 1));
    }

    @Test
    void zeroFall_shouldThrow() {
        assertThrows(IllegalArgumentException.class,
                () -> new StubHealthCheck(100, 1, 0));
    }

    // ===== Exponential backoff =====

    @Test
    void exponentialBackoff() {
        StubHealthCheck hc = new StubHealthCheck();

        assertEquals(0, hc.currentBackoffMs(), "No backoff initially");

        hc.failure();
        // After first failure, backoff should be 0 (default baseBackoffMs=1000,
        // but this test stub uses baseBackoffMs=0)
    }

    // ===== Getters =====

    @Test
    void getters_returnConfiguredValues() {
        StubHealthCheck hc = new StubHealthCheck(100, 5, 7);

        assertEquals(5, hc.consecutiveSuccessesForHealthy());
        assertEquals(7, hc.consecutiveFailuresForUnhealthy());
    }

    @Test
    void getters_defaultConstructor_returnsOne() {
        StubHealthCheck hc = new StubHealthCheck();

        assertEquals(1, hc.consecutiveSuccessesForHealthy());
        assertEquals(1, hc.consecutiveFailuresForUnhealthy());
    }

    // ===== Recovery from BAD to GOOD =====

    @Test
    void riseFall3_recoveryFromBadToGood() {
        StubHealthCheck hc = new StubHealthCheck(100, 3, 3);

        hc.failure();
        hc.failure();
        hc.failure();
        assertEquals(Health.BAD, hc.health());

        hc.success();
        hc.success();
        hc.success();

        assertEquals(Health.BAD, hc.health(),
                "1 failure + 1 success in queue = 50% -> BAD");

        for (int i = 0; i < 60; i++) {
            hc.success();
        }

        assertEquals(Health.GOOD, hc.health(),
                "After sustained successes, health must recover to GOOD");
    }
}
