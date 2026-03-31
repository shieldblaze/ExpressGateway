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

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class CircuitBreakerTest {

    @Test
    void startsInClosedState() {
        CircuitBreaker cb = new CircuitBreaker(3, Duration.ofSeconds(5));
        assertEquals(CircuitBreaker.State.CLOSED, cb.state());
        assertEquals(0, cb.consecutiveFailures());
    }

    @Test
    void allowsRequestsInClosedState() {
        CircuitBreaker cb = new CircuitBreaker(3, Duration.ofSeconds(5));
        assertDoesNotThrow(cb::allowRequest);
    }

    @Test
    void opensAfterThresholdFailures() {
        CircuitBreaker cb = new CircuitBreaker(3, Duration.ofSeconds(30));
        cb.recordFailure();
        assertEquals(CircuitBreaker.State.CLOSED, cb.state());
        cb.recordFailure();
        assertEquals(CircuitBreaker.State.CLOSED, cb.state());
        cb.recordFailure();
        assertEquals(CircuitBreaker.State.OPEN, cb.state());
    }

    @Test
    void rejectsRequestsWhenOpen() {
        CircuitBreaker cb = new CircuitBreaker(1, Duration.ofSeconds(30));
        cb.recordFailure();
        assertEquals(CircuitBreaker.State.OPEN, cb.state());
        assertThrows(CircuitBreakerOpenException.class, cb::allowRequest);
    }

    @Test
    void successResetsToClosedState() {
        CircuitBreaker cb = new CircuitBreaker(2, Duration.ofSeconds(5));
        cb.recordFailure();
        assertEquals(1, cb.consecutiveFailures());
        cb.recordSuccess();
        assertEquals(CircuitBreaker.State.CLOSED, cb.state());
        assertEquals(0, cb.consecutiveFailures());
    }

    @Test
    void transitionsToHalfOpenAfterTimeout() throws InterruptedException {
        CircuitBreaker cb = new CircuitBreaker(1, Duration.ofMillis(50));
        cb.recordFailure();
        assertEquals(CircuitBreaker.State.OPEN, cb.state());

        // Wait for reset timeout
        Thread.sleep(100);

        // Should transition to HALF_OPEN
        assertDoesNotThrow(cb::allowRequest);
        assertEquals(CircuitBreaker.State.HALF_OPEN, cb.state());
    }

    @Test
    void halfOpenSuccessClosesCircuit() throws InterruptedException {
        CircuitBreaker cb = new CircuitBreaker(1, Duration.ofMillis(50));
        cb.recordFailure();
        Thread.sleep(100);
        cb.allowRequest(); // transitions to HALF_OPEN
        cb.recordSuccess();
        assertEquals(CircuitBreaker.State.CLOSED, cb.state());
    }

    @Test
    void halfOpenFailureReopensCircuit() throws InterruptedException {
        CircuitBreaker cb = new CircuitBreaker(1, Duration.ofMillis(50));
        cb.recordFailure();
        Thread.sleep(100);
        cb.allowRequest(); // transitions to HALF_OPEN
        cb.recordFailure();
        assertEquals(CircuitBreaker.State.OPEN, cb.state());
    }

    @Test
    void resetClearsState() {
        CircuitBreaker cb = new CircuitBreaker(1, Duration.ofSeconds(30));
        cb.recordFailure();
        assertEquals(CircuitBreaker.State.OPEN, cb.state());
        cb.reset();
        assertEquals(CircuitBreaker.State.CLOSED, cb.state());
        assertEquals(0, cb.consecutiveFailures());
    }

    @Test
    void invalidThresholdThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> new CircuitBreaker(0, Duration.ofSeconds(5)));
    }
}
