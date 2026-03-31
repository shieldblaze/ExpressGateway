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

import com.shieldblaze.expressgateway.backend.CircuitBreaker.CircuitState;
import com.shieldblaze.expressgateway.configuration.healthcheck.CircuitBreakerConfiguration;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.lang.reflect.Constructor;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * V-TEST-022: Circuit breaker state transition tests.
 *
 * <p>Validates the full state machine:
 * <pre>
 *   CLOSED --(failureThreshold consecutive failures)--> OPEN
 *   OPEN   --(openDuration elapsed)--------------------> HALF_OPEN
 *   HALF_OPEN --(successThreshold successes)-----------> CLOSED
 *   HALF_OPEN --(any failure)--------------------------> OPEN
 * </pre>
 *
 * <p>Uses a short openDurationMs (50ms) to make OPEN -> HALF_OPEN transitions
 * testable without long sleeps. All tests are deterministic and self-contained.</p>
 *
 * <p>The {@link CircuitBreakerConfiguration} constructor is package-private, so this
 * test creates instances via reflection -- standard practice for testing configuration
 * objects from a different package.</p>
 */
@Timeout(value = 30)
class CircuitBreakerStateTest {

    private CircuitBreakerConfiguration config;

    /**
     * Creates a CircuitBreakerConfiguration via reflection since the constructor
     * is package-private.
     */
    private static CircuitBreakerConfiguration createConfig(boolean enabled, int failureThreshold,
                                                            int successThreshold, long openDurationMs,
                                                            int halfOpenMaxConcurrent) throws Exception {
        Constructor<CircuitBreakerConfiguration> ctor =
                CircuitBreakerConfiguration.class.getDeclaredConstructor();
        ctor.setAccessible(true);
        return ctor.newInstance()
                .setEnabled(enabled)
                .setFailureThreshold(failureThreshold)
                .setSuccessThreshold(successThreshold)
                .setOpenDurationMs(openDurationMs)
                .setHalfOpenMaxConcurrent(halfOpenMaxConcurrent)
                .validate();
    }

    @BeforeEach
    void setUp() throws Exception {
        config = createConfig(true, 3, 2, 50, 1);
    }

    // ===================================================================
    // Basic state transitions
    // ===================================================================

    /**
     * CLOSED is the initial state and all requests are allowed.
     */
    @Test
    void initialState_isClosed_andAllowsRequests() {
        CircuitBreaker cb = new CircuitBreaker(config);
        assertEquals(CircuitState.CLOSED, cb.state());
        assertTrue(cb.allowRequest());
    }

    /**
     * CLOSED -> OPEN: After failureThreshold (3) consecutive failures,
     * the circuit opens and rejects requests.
     */
    @Test
    void closedToOpen_afterConsecutiveFailures() {
        CircuitBreaker cb = new CircuitBreaker(config);

        cb.recordFailure();
        assertEquals(CircuitState.CLOSED, cb.state(), "1 failure: still CLOSED");
        assertTrue(cb.allowRequest());

        cb.recordFailure();
        assertEquals(CircuitState.CLOSED, cb.state(), "2 failures: still CLOSED");
        assertTrue(cb.allowRequest());

        cb.recordFailure();
        assertEquals(CircuitState.OPEN, cb.state(), "3 failures: must transition to OPEN");
        assertFalse(cb.allowRequest(), "OPEN circuit must reject requests");
    }

    /**
     * A success in CLOSED state resets the consecutive failure counter,
     * preventing the circuit from opening.
     */
    @Test
    void successInClosed_resetsFailureCounter() {
        CircuitBreaker cb = new CircuitBreaker(config);

        cb.recordFailure();
        cb.recordFailure();
        assertEquals(CircuitState.CLOSED, cb.state(), "2 failures: still CLOSED");

        // Success resets consecutive failures
        cb.recordSuccess();

        // Need failureThreshold (3) NEW consecutive failures to open
        cb.recordFailure();
        cb.recordFailure();
        assertEquals(CircuitState.CLOSED, cb.state(), "2 new failures after reset: still CLOSED");

        cb.recordFailure();
        assertEquals(CircuitState.OPEN, cb.state(), "3 new failures: now OPEN");
    }

    /**
     * OPEN -> HALF_OPEN: After openDurationMs elapses, the next allowRequest()
     * transitions to HALF_OPEN and permits a probe request.
     */
    @Test
    void openToHalfOpen_afterOpenDurationExpires() throws Exception {
        CircuitBreaker cb = new CircuitBreaker(config);

        // Force to OPEN
        cb.recordFailure();
        cb.recordFailure();
        cb.recordFailure();
        assertEquals(CircuitState.OPEN, cb.state());

        // Wait for openDurationMs (50ms) + margin
        Thread.sleep(100);

        // The next allowRequest() should transition to HALF_OPEN
        assertTrue(cb.allowRequest(), "After open duration, allowRequest must succeed (HALF_OPEN probe)");
        assertEquals(CircuitState.HALF_OPEN, cb.state());
    }

    /**
     * HALF_OPEN -> CLOSED: After successThreshold (2) consecutive successes
     * in HALF_OPEN state, the circuit closes and resumes normal operation.
     */
    @Test
    void halfOpenToClosed_afterSuccessThreshold() throws Exception {
        CircuitBreaker cb = new CircuitBreaker(config);

        // CLOSED -> OPEN
        cb.recordFailure();
        cb.recordFailure();
        cb.recordFailure();
        assertEquals(CircuitState.OPEN, cb.state());

        // OPEN -> HALF_OPEN
        Thread.sleep(100);
        assertTrue(cb.allowRequest());
        assertEquals(CircuitState.HALF_OPEN, cb.state());

        // Record successes to close the circuit
        cb.recordSuccess();
        assertEquals(CircuitState.HALF_OPEN, cb.state(), "1 success: still HALF_OPEN");

        cb.recordSuccess();
        assertEquals(CircuitState.CLOSED, cb.state(), "2 successes: must transition to CLOSED");

        // Normal operation resumes
        assertTrue(cb.allowRequest());
    }

    /**
     * HALF_OPEN -> OPEN: Any failure in HALF_OPEN immediately re-opens the circuit.
     */
    @Test
    void halfOpenToOpen_onAnyFailure() throws Exception {
        CircuitBreaker cb = new CircuitBreaker(config);

        // CLOSED -> OPEN
        cb.recordFailure();
        cb.recordFailure();
        cb.recordFailure();

        // OPEN -> HALF_OPEN
        Thread.sleep(100);
        cb.allowRequest();
        assertEquals(CircuitState.HALF_OPEN, cb.state());

        // Any failure in HALF_OPEN -> OPEN
        cb.recordFailure();
        assertEquals(CircuitState.OPEN, cb.state(), "Failure in HALF_OPEN must re-open the circuit");
        assertFalse(cb.allowRequest(), "Re-opened circuit must reject requests immediately");
    }

    /**
     * Full cycle: CLOSED -> OPEN -> HALF_OPEN -> OPEN -> HALF_OPEN -> CLOSED.
     * Tests that the circuit breaker can complete multiple full transitions.
     */
    @Test
    void fullCycle_closedOpenHalfOpenOpenHalfOpenClosed() throws Exception {
        CircuitBreaker cb = new CircuitBreaker(config);

        // CLOSED -> OPEN (3 failures)
        for (int i = 0; i < 3; i++) {
            cb.recordFailure();
        }
        assertEquals(CircuitState.OPEN, cb.state());

        // OPEN -> HALF_OPEN (wait for open duration)
        Thread.sleep(100);
        cb.allowRequest();
        assertEquals(CircuitState.HALF_OPEN, cb.state());

        // HALF_OPEN -> OPEN (failure)
        cb.recordFailure();
        assertEquals(CircuitState.OPEN, cb.state());

        // OPEN -> HALF_OPEN again
        Thread.sleep(100);
        cb.allowRequest();
        assertEquals(CircuitState.HALF_OPEN, cb.state());

        // HALF_OPEN -> CLOSED (2 successes)
        cb.recordSuccess();
        cb.recordSuccess();
        assertEquals(CircuitState.CLOSED, cb.state());

        // Fully operational
        assertTrue(cb.allowRequest());
    }

    // ===================================================================
    // HALF_OPEN concurrency limiting
    // ===================================================================

    /**
     * HALF_OPEN state limits concurrent requests to halfOpenMaxConcurrent (1).
     * The first allowRequest() returns true; subsequent calls return false
     * until the circuit transitions.
     */
    @Test
    void halfOpen_limitsMaxConcurrentRequests() throws Exception {
        CircuitBreaker cb = new CircuitBreaker(config);

        // CLOSED -> OPEN -> HALF_OPEN
        for (int i = 0; i < 3; i++) {
            cb.recordFailure();
        }
        Thread.sleep(100);

        // First probe request allowed (transitions OPEN -> HALF_OPEN)
        assertTrue(cb.allowRequest(), "First probe request must be allowed");
        assertEquals(CircuitState.HALF_OPEN, cb.state());

        // Second concurrent request exceeds halfOpenMaxConcurrent (1)
        assertFalse(cb.allowRequest(), "Second concurrent request in HALF_OPEN must be rejected");
    }

    /**
     * HALF_OPEN with higher concurrency limit allows multiple probe requests.
     */
    @Test
    void halfOpen_withHigherConcurrencyLimit_allowsMultipleProbes() throws Exception {
        CircuitBreakerConfiguration highConcurrencyConfig = createConfig(true, 3, 2, 50, 3);

        CircuitBreaker cb = new CircuitBreaker(highConcurrencyConfig);

        // CLOSED -> OPEN -> HALF_OPEN
        for (int i = 0; i < 3; i++) {
            cb.recordFailure();
        }
        Thread.sleep(100);

        // Up to 3 concurrent requests allowed
        assertTrue(cb.allowRequest(), "Probe request 1/3");
        assertTrue(cb.allowRequest(), "Probe request 2/3");
        assertTrue(cb.allowRequest(), "Probe request 3/3");
        assertFalse(cb.allowRequest(), "4th concurrent request must be rejected");
    }

    // ===================================================================
    // Administrative controls
    // ===================================================================

    /**
     * forceState() allows administrative override of the circuit state.
     */
    @Test
    void forceState_overridesCurrentState() {
        CircuitBreaker cb = new CircuitBreaker(config);
        assertEquals(CircuitState.CLOSED, cb.state());

        cb.forceState(CircuitState.OPEN);
        assertEquals(CircuitState.OPEN, cb.state());
        assertFalse(cb.allowRequest(), "Forced OPEN must reject requests");

        cb.forceState(CircuitState.CLOSED);
        assertEquals(CircuitState.CLOSED, cb.state());
        assertTrue(cb.allowRequest(), "Forced CLOSED must allow requests");

        cb.forceState(CircuitState.HALF_OPEN);
        assertEquals(CircuitState.HALF_OPEN, cb.state());
    }

    /**
     * reset() returns the circuit to CLOSED with zeroed counters.
     */
    @Test
    void reset_returnsToClosed() {
        CircuitBreaker cb = new CircuitBreaker(config);

        // Get into OPEN state
        cb.recordFailure();
        cb.recordFailure();
        cb.recordFailure();
        assertEquals(CircuitState.OPEN, cb.state());

        cb.reset();
        assertEquals(CircuitState.CLOSED, cb.state());
        assertTrue(cb.allowRequest());

        // Failure counter was also reset -- need 3 new failures to open
        cb.recordFailure();
        cb.recordFailure();
        assertEquals(CircuitState.CLOSED, cb.state(), "After reset, need full failureThreshold again");
    }

    // ===================================================================
    // Disabled circuit breaker
    // ===================================================================

    /**
     * When disabled, all requests are allowed regardless of failures.
     */
    @Test
    void disabled_alwaysAllowsRequests() throws Exception {
        CircuitBreakerConfiguration disabledConfig = createConfig(false, 1, 1, 1, 1);

        CircuitBreaker cb = new CircuitBreaker(disabledConfig);

        // Record many failures -- should have no effect
        for (int i = 0; i < 100; i++) {
            cb.recordFailure();
        }

        assertEquals(CircuitState.CLOSED, cb.state(), "Disabled CB must stay CLOSED");
        assertTrue(cb.allowRequest(), "Disabled CB must always allow requests");
    }

    // ===================================================================
    // Thread safety
    // ===================================================================

    /**
     * Concurrent calls to recordFailure/recordSuccess/allowRequest must not
     * corrupt the state machine. The circuit breaker uses CAS-based state
     * transitions, so this test validates that no IllegalStateException or
     * inconsistent state occurs under contention.
     */
    @Test
    void concurrentAccess_noCorruption() throws Exception {
        CircuitBreakerConfiguration fastConfig = createConfig(true, 5, 3, 10, 2);

        CircuitBreaker cb = new CircuitBreaker(fastConfig);
        int threadCount = 8;
        int iterationsPerThread = 1000;
        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        AtomicInteger errors = new AtomicInteger(0);

        for (int t = 0; t < threadCount; t++) {
            final int threadId = t;
            executor.submit(() -> {
                try {
                    startLatch.await();
                    for (int i = 0; i < iterationsPerThread; i++) {
                        cb.allowRequest();
                        if (threadId % 2 == 0) {
                            cb.recordFailure();
                        } else {
                            cb.recordSuccess();
                        }
                        // Validate state is always a valid enum value
                        CircuitState s = cb.state();
                        if (s != CircuitState.CLOSED && s != CircuitState.OPEN && s != CircuitState.HALF_OPEN) {
                            errors.incrementAndGet();
                        }
                    }
                } catch (Exception e) {
                    errors.incrementAndGet();
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        startLatch.countDown();
        assertTrue(doneLatch.await(10, TimeUnit.SECONDS), "All threads must complete");
        assertEquals(0, errors.get(), "No errors during concurrent access");

        // State must be valid after contention
        CircuitState finalState = cb.state();
        assertTrue(
                finalState == CircuitState.CLOSED ||
                        finalState == CircuitState.OPEN ||
                        finalState == CircuitState.HALF_OPEN,
                "Final state must be valid: " + finalState
        );

        executor.shutdown();
    }
}
