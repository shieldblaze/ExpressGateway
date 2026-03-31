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
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Three-state circuit breaker (CLOSED -> OPEN -> HALF_OPEN -> CLOSED) for
 * protecting discovery calls from cascading failures. Thread-safe and lock-free.
 *
 * <p>State transitions:</p>
 * <ul>
 *   <li>CLOSED: requests flow normally. After {@code failureThreshold} consecutive failures, transitions to OPEN.</li>
 *   <li>OPEN: all requests fail fast with {@link CircuitBreakerOpenException}. After {@code resetTimeout}, transitions to HALF_OPEN.</li>
 *   <li>HALF_OPEN: one trial request is allowed. On success, transitions to CLOSED. On failure, transitions back to OPEN.</li>
 * </ul>
 */
public final class CircuitBreaker {

    /**
     * Circuit breaker states.
     */
    public enum State { CLOSED, OPEN, HALF_OPEN }

    private final int failureThreshold;
    private final long resetTimeoutMillis;
    private final AtomicReference<State> state = new AtomicReference<>(State.CLOSED);
    private final AtomicInteger consecutiveFailures = new AtomicInteger(0);
    private final AtomicLong lastFailureTimestamp = new AtomicLong(0);

    /**
     * @param failureThreshold number of consecutive failures before opening the circuit
     * @param resetTimeout     time to wait in OPEN state before allowing a trial request
     */
    public CircuitBreaker(int failureThreshold, Duration resetTimeout) {
        if (failureThreshold < 1) {
            throw new IllegalArgumentException("failureThreshold must be >= 1");
        }
        this.failureThreshold = failureThreshold;
        this.resetTimeoutMillis = resetTimeout.toMillis();
    }

    /**
     * Check if a request is allowed. Throws if the circuit is open and
     * the reset timeout has not yet elapsed.
     *
     * @throws CircuitBreakerOpenException if the circuit is open
     */
    public void allowRequest() {
        State current = state.get();
        if (current == State.OPEN) {
            long elapsed = System.currentTimeMillis() - lastFailureTimestamp.get();
            if (elapsed >= resetTimeoutMillis) {
                // Transition to HALF_OPEN: allow one trial request
                state.compareAndSet(State.OPEN, State.HALF_OPEN);
            } else {
                throw new CircuitBreakerOpenException(
                        "Circuit breaker is OPEN. Will retry in " + (resetTimeoutMillis - elapsed) + "ms");
            }
        }
        // CLOSED and HALF_OPEN allow requests
    }

    /**
     * Record a successful call. Resets the circuit to CLOSED.
     */
    public void recordSuccess() {
        consecutiveFailures.set(0);
        state.set(State.CLOSED);
    }

    /**
     * Record a failed call. May transition the circuit to OPEN.
     */
    public void recordFailure() {
        lastFailureTimestamp.set(System.currentTimeMillis());
        int failures = consecutiveFailures.incrementAndGet();

        State current = state.get();
        if (current == State.HALF_OPEN) {
            // Trial request failed; back to OPEN
            state.set(State.OPEN);
        } else if (failures >= failureThreshold) {
            state.set(State.OPEN);
        }
    }

    /**
     * Return the current circuit breaker state.
     */
    public State state() {
        return state.get();
    }

    /**
     * Return the count of consecutive failures.
     */
    public int consecutiveFailures() {
        return consecutiveFailures.get();
    }

    /**
     * Reset the circuit breaker to the CLOSED state.
     */
    public void reset() {
        consecutiveFailures.set(0);
        lastFailureTimestamp.set(0);
        state.set(State.CLOSED);
    }
}
