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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.configuration.healthcheck.CircuitBreakerConfiguration;
import lombok.extern.log4j.Log4j2;

import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Per-node circuit breaker that prevents cascading failures by temporarily
 * stopping traffic to unhealthy backends.
 *
 * <p>State machine:</p>
 * <pre>
 *   CLOSED --(failureThreshold consecutive failures)--> OPEN
 *   OPEN   --(openDuration elapsed)--------------------> HALF_OPEN
 *   HALF_OPEN --(successThreshold successes)-----------> CLOSED
 *   HALF_OPEN --(any failure)--------------------------> OPEN
 * </pre>
 *
 * <p>Thread-safe: uses AtomicReference for state and AtomicInteger for counters.
 * All state transitions are performed via CAS to avoid external synchronization.</p>
 */
@Log4j2
public final class CircuitBreaker {

    /**
     * Circuit breaker states
     */
    public enum CircuitState {
        /** Normal operation - traffic flows freely */
        CLOSED,
        /** Circuit is open - traffic is rejected */
        OPEN,
        /** Testing state - limited traffic allowed to probe backend health */
        HALF_OPEN
    }

    private final CircuitBreakerConfiguration config;
    private final AtomicReference<CircuitState> state = new AtomicReference<>(CircuitState.CLOSED);
    private final AtomicInteger consecutiveFailures = new AtomicInteger(0);
    private final AtomicInteger consecutiveSuccesses = new AtomicInteger(0);
    private final AtomicInteger halfOpenInFlight = new AtomicInteger(0);

    /**
     * Timestamp (System.nanoTime()) when the circuit was opened.
     * Volatile because it is written in one CAS-guarded path and read in another.
     */
    private volatile long openedAtNanos;

    public CircuitBreaker(CircuitBreakerConfiguration config) {
        this.config = config;
    }

    /**
     * Check if a request is allowed through the circuit breaker.
     *
     * @return {@code true} if the request should proceed, {@code false} if it should be rejected
     */
    public boolean allowRequest() {
        if (!config.enabled()) {
            return true;
        }

        // HIGH-06: Re-read state after CAS attempts to handle races correctly.
        // A thread that loses the CAS must re-check the state to determine
        // whether the winner already transitioned to HALF_OPEN.
        CircuitState currentState = state.get();
        return switch (currentState) {
            case CLOSED -> true;
            case OPEN -> {
                // Check if enough time has elapsed to transition to HALF_OPEN
                long elapsedMs = (System.nanoTime() - openedAtNanos) / 1_000_000;
                if (elapsedMs >= config.openDurationMs()) {
                    // Attempt transition from OPEN -> HALF_OPEN.
                    // HIGH-06: Reset counters BEFORE the CAS succeeds, then atomically
                    // increment. This eliminates the window where a concurrent thread
                    // sees HALF_OPEN state but stale counter values.
                    if (state.compareAndSet(CircuitState.OPEN, CircuitState.HALF_OPEN)) {
                        consecutiveSuccesses.set(0);
                        // Reset and claim the first slot atomically: set to 1 instead of
                        // set(0) + incrementAndGet() to avoid a window where another thread
                        // increments between set(0) and our own increment.
                        halfOpenInFlight.set(1);
                        log.info("Circuit breaker transitioning from OPEN to HALF_OPEN after {}ms", elapsedMs);
                        yield 1 <= config.halfOpenMaxConcurrent();
                    }
                    // CAS lost: another thread already transitioned. Fall through to
                    // re-check the (now possibly HALF_OPEN) state.
                    currentState = state.get();
                    if (currentState == CircuitState.HALF_OPEN) {
                        yield halfOpenInFlight.incrementAndGet() <= config.halfOpenMaxConcurrent();
                    }
                    // State changed to something else (e.g., back to OPEN or CLOSED).
                    yield currentState == CircuitState.CLOSED;
                }
                yield false;
            }
            case HALF_OPEN -> {
                // In HALF_OPEN, allow limited concurrent requests
                yield halfOpenInFlight.incrementAndGet() <= config.halfOpenMaxConcurrent();
            }
        };
    }

    /**
     * Record a successful request.
     * In HALF_OPEN state, accumulates successes toward closing the circuit.
     */
    public void recordSuccess() {
        if (!config.enabled()) {
            return;
        }

        consecutiveFailures.set(0);

        CircuitState currentState = state.get();
        if (currentState == CircuitState.HALF_OPEN) {
            int successes = consecutiveSuccesses.incrementAndGet();
            if (successes >= config.successThreshold()) {
                // Enough successes to close the circuit
                if (state.compareAndSet(CircuitState.HALF_OPEN, CircuitState.CLOSED)) {
                    consecutiveSuccesses.set(0);
                    halfOpenInFlight.set(0);
                    log.info("Circuit breaker CLOSED after {} consecutive successes", successes);
                }
            }
        }
    }

    /**
     * Record a failed request.
     * In CLOSED state, accumulates failures toward opening the circuit.
     * In HALF_OPEN state, any failure immediately opens the circuit.
     */
    public void recordFailure() {
        if (!config.enabled()) {
            return;
        }

        consecutiveSuccesses.set(0);

        CircuitState currentState = state.get();
        if (currentState == CircuitState.HALF_OPEN) {
            // Any failure in HALF_OPEN -> immediately go back to OPEN
            if (state.compareAndSet(CircuitState.HALF_OPEN, CircuitState.OPEN)) {
                openedAtNanos = System.nanoTime();
                halfOpenInFlight.set(0);
                log.warn("Circuit breaker re-opened from HALF_OPEN after failure");
            }
        } else if (currentState == CircuitState.CLOSED) {
            int failures = consecutiveFailures.incrementAndGet();
            if (failures >= config.failureThreshold()) {
                // Enough failures to open the circuit
                if (state.compareAndSet(CircuitState.CLOSED, CircuitState.OPEN)) {
                    openedAtNanos = System.nanoTime();
                    consecutiveFailures.set(0);
                    log.warn("Circuit breaker OPENED after {} consecutive failures", failures);
                }
            }
        }
    }

    /**
     * Returns the current circuit state
     */
    public CircuitState state() {
        return state.get();
    }

    /**
     * Force the circuit to a specific state. Intended for administrative override.
     */
    public void forceState(CircuitState newState) {
        CircuitState old = state.getAndSet(newState);
        consecutiveFailures.set(0);
        consecutiveSuccesses.set(0);
        halfOpenInFlight.set(0);
        if (newState == CircuitState.OPEN) {
            openedAtNanos = System.nanoTime();
        }
        log.info("Circuit breaker forced from {} to {}", old, newState);
    }

    /**
     * Reset the circuit breaker to CLOSED state with zeroed counters
     */
    public void reset() {
        state.set(CircuitState.CLOSED);
        consecutiveFailures.set(0);
        consecutiveSuccesses.set(0);
        halfOpenInFlight.set(0);
    }

    @Override
    public String toString() {
        return "CircuitBreaker{state=" + state.get() +
                ", failures=" + consecutiveFailures.get() +
                ", successes=" + consecutiveSuccesses.get() + '}';
    }
}
