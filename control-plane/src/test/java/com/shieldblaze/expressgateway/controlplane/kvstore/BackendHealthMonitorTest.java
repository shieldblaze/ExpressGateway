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
package com.shieldblaze.expressgateway.controlplane.kvstore;

import com.shieldblaze.expressgateway.controlplane.testutil.InMemoryKVStore;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link BackendHealthMonitor} using an {@link InMemoryKVStore}
 * that can be made to fail on demand.
 *
 * <p>Tests verify the three-state health model (HEALTHY, DEGRADED, UNHEALTHY),
 * state transition callbacks, and recovery behavior.</p>
 */
class BackendHealthMonitorTest {

    private BackendHealthMonitor monitor;

    @AfterEach
    void tearDown() {
        if (monitor != null) {
            monitor.close();
            monitor = null;
        }
    }

    // ===========================================================================
    // Test: HEALTHY state on start with healthy backend
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Monitor starts in HEALTHY state with a healthy backend")
    void testHealthyStateOnStart() throws Exception {
        InMemoryKVStore store = new InMemoryKVStore();
        monitor = new BackendHealthMonitor(store, 500, 5000);
        monitor.start();

        // Wait for at least one check cycle to complete
        Thread.sleep(1500);

        assertEquals(BackendHealthMonitor.HealthState.HEALTHY, monitor.currentState(),
                "Monitor should be HEALTHY with a working backend");
        assertTrue(monitor.isHealthy(), "isHealthy() should return true");
    }

    // ===========================================================================
    // Test: Transition to UNHEALTHY when backend fails
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Monitor transitions to UNHEALTHY when backend starts failing")
    void testTransitionToUnhealthy() throws Exception {
        InMemoryKVStore store = new InMemoryKVStore();
        monitor = new BackendHealthMonitor(store, 500, 5000);

        CountDownLatch unhealthyLatch = new CountDownLatch(1);
        monitor.addListener((prev, next, latencyMs) -> {
            if (next == BackendHealthMonitor.HealthState.UNHEALTHY) {
                unhealthyLatch.countDown();
            }
        });

        monitor.start();

        // Wait for initial HEALTHY state
        Thread.sleep(1500);
        assertEquals(BackendHealthMonitor.HealthState.HEALTHY, monitor.currentState());

        // Trigger failures
        store.setFailOnWrite(true);

        // Wait for UNHEALTHY transition
        assertTrue(unhealthyLatch.await(10, TimeUnit.SECONDS),
                "Monitor should transition to UNHEALTHY when backend fails");
        assertEquals(BackendHealthMonitor.HealthState.UNHEALTHY, monitor.currentState());
        assertFalse(monitor.isHealthy(), "isHealthy() should return false");
    }

    // ===========================================================================
    // Test: Recovery back to HEALTHY
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Monitor recovers to HEALTHY after backend becomes available again")
    void testRecoveryToHealthy() throws Exception {
        InMemoryKVStore store = new InMemoryKVStore();
        monitor = new BackendHealthMonitor(store, 500, 5000);

        CountDownLatch unhealthyLatch = new CountDownLatch(1);
        CountDownLatch recoveredLatch = new CountDownLatch(1);

        monitor.addListener((prev, next, latencyMs) -> {
            if (next == BackendHealthMonitor.HealthState.UNHEALTHY) {
                unhealthyLatch.countDown();
            }
            if (next == BackendHealthMonitor.HealthState.HEALTHY
                    && prev == BackendHealthMonitor.HealthState.UNHEALTHY) {
                recoveredLatch.countDown();
            }
        });

        monitor.start();

        // Wait for initial HEALTHY
        Thread.sleep(1500);

        // Break the backend
        store.setFailOnWrite(true);
        assertTrue(unhealthyLatch.await(10, TimeUnit.SECONDS),
                "Should transition to UNHEALTHY");

        // Fix the backend
        store.setFailOnWrite(false);
        assertTrue(recoveredLatch.await(10, TimeUnit.SECONDS),
                "Should recover to HEALTHY after backend is fixed");

        assertEquals(BackendHealthMonitor.HealthState.HEALTHY, monitor.currentState());
        assertTrue(monitor.isHealthy());
    }

    // ===========================================================================
    // Test: State listener callbacks fire correctly
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Listener receives correct previous and new state on transition")
    void testListenerCallbackArguments() throws Exception {
        InMemoryKVStore store = new InMemoryKVStore();
        monitor = new BackendHealthMonitor(store, 500, 5000);

        CountDownLatch latch = new CountDownLatch(1);
        AtomicReference<BackendHealthMonitor.HealthState> prevRef = new AtomicReference<>();
        AtomicReference<BackendHealthMonitor.HealthState> nextRef = new AtomicReference<>();

        monitor.addListener((prev, next, latencyMs) -> {
            if (next == BackendHealthMonitor.HealthState.UNHEALTHY) {
                prevRef.set(prev);
                nextRef.set(next);
                latch.countDown();
            }
        });

        monitor.start();
        Thread.sleep(1500);

        // Break the backend
        store.setFailOnWrite(true);
        assertTrue(latch.await(10, TimeUnit.SECONDS));

        assertEquals(BackendHealthMonitor.HealthState.HEALTHY, prevRef.get(),
                "Previous state should be HEALTHY");
        assertEquals(BackendHealthMonitor.HealthState.UNHEALTHY, nextRef.get(),
                "New state should be UNHEALTHY");
    }

    // ===========================================================================
    // Test: Listener removal
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Removed listener does not receive subsequent events")
    void testListenerRemoval() throws Exception {
        InMemoryKVStore store = new InMemoryKVStore();
        monitor = new BackendHealthMonitor(store, 500, 5000);

        CountDownLatch shouldNotFire = new CountDownLatch(1);
        BackendHealthMonitor.HealthStateListener listener = (prev, next, latencyMs) -> {
            shouldNotFire.countDown();
        };

        monitor.addListener(listener);
        boolean removed = monitor.removeListener(listener);
        assertTrue(removed, "removeListener should return true for a registered listener");

        monitor.start();

        // Break the backend -- if the listener was still active, it would fire
        store.setFailOnWrite(true);
        Thread.sleep(3000);

        assertEquals(1, shouldNotFire.getCount(),
                "Removed listener should not have been called");
    }

    // ===========================================================================
    // Test: Multiple listeners all receive events
    // ===========================================================================

    @Test
    @Timeout(30)
    @DisplayName("Multiple listeners all receive state change events")
    void testMultipleListeners() throws Exception {
        InMemoryKVStore store = new InMemoryKVStore();
        monitor = new BackendHealthMonitor(store, 500, 5000);

        CountDownLatch listener1Latch = new CountDownLatch(1);
        CountDownLatch listener2Latch = new CountDownLatch(1);

        monitor.addListener((prev, next, latencyMs) -> {
            if (next == BackendHealthMonitor.HealthState.UNHEALTHY) {
                listener1Latch.countDown();
            }
        });
        monitor.addListener((prev, next, latencyMs) -> {
            if (next == BackendHealthMonitor.HealthState.UNHEALTHY) {
                listener2Latch.countDown();
            }
        });

        monitor.start();
        Thread.sleep(1500);

        store.setFailOnWrite(true);

        assertTrue(listener1Latch.await(10, TimeUnit.SECONDS), "Listener 1 should fire");
        assertTrue(listener2Latch.await(10, TimeUnit.SECONDS), "Listener 2 should fire");
    }

    // ===========================================================================
    // Test: Constructor validation
    // ===========================================================================

    @Test
    @DisplayName("Constructor rejects non-positive intervalMs")
    void testInvalidIntervalMs() {
        InMemoryKVStore store = new InMemoryKVStore();
        assertThrows(IllegalArgumentException.class,
                () -> new BackendHealthMonitor(store, 0, 500));
    }

    @Test
    @DisplayName("Constructor rejects non-positive maxLatencyMs")
    void testInvalidMaxLatencyMs() {
        InMemoryKVStore store = new InMemoryKVStore();
        assertThrows(IllegalArgumentException.class,
                () -> new BackendHealthMonitor(store, 1000, 0));
    }

    @Test
    @DisplayName("Constructor rejects null store")
    void testNullStore() {
        assertThrows(NullPointerException.class,
                () -> new BackendHealthMonitor(null, 1000, 500));
    }

    // ===========================================================================
    // Test: Close is idempotent
    // ===========================================================================

    @Test
    @Timeout(10)
    @DisplayName("close() is idempotent and does not throw on multiple calls")
    void testCloseIdempotent() {
        InMemoryKVStore store = new InMemoryKVStore();
        monitor = new BackendHealthMonitor(store, 500, 5000);
        monitor.start();

        monitor.close();
        monitor.close(); // Should not throw
        monitor = null;  // Prevent double-close in tearDown
    }

    // ===========================================================================
    // Test: start() after close() throws
    // ===========================================================================

    @Test
    @DisplayName("start() after close() throws IllegalStateException")
    void testStartAfterClose() {
        InMemoryKVStore store = new InMemoryKVStore();
        monitor = new BackendHealthMonitor(store, 500, 5000);
        monitor.start();
        monitor.close();

        assertThrows(IllegalStateException.class, monitor::start);
        monitor = null; // Prevent double-close in tearDown
    }
}
