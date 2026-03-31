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

import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.nio.charset.StandardCharsets;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ThreadFactory;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Runtime health monitor that continuously checks KV store backend health.
 *
 * <p>Unlike {@link BackendHealthChecker} (which runs once at startup), this monitor
 * runs periodically on a configurable interval and tracks a three-state health model:</p>
 * <ul>
 *     <li>{@link HealthState#HEALTHY} -- backend is reachable and responding within latency bounds</li>
 *     <li>{@link HealthState#DEGRADED} -- backend is reachable but latency exceeds the configured maximum</li>
 *     <li>{@link HealthState#UNHEALTHY} -- backend is unreachable or a read/write cycle failed</li>
 * </ul>
 *
 * <p>State transitions fire registered {@link HealthStateListener} callbacks synchronously
 * on the scheduler thread. Listeners must not block.</p>
 *
 * <p>Thread safety: all public methods are safe for concurrent use. The monitor uses a single
 * daemon thread for periodic checks.</p>
 */
@Log4j2
public final class BackendHealthMonitor implements Closeable {

    /**
     * Three-state health model for the KV store backend.
     */
    public enum HealthState {
        /** Backend is reachable and responding within latency bounds. */
        HEALTHY,
        /** Backend is reachable but latency exceeds the configured maximum. */
        DEGRADED,
        /** Backend is unreachable or a read/write cycle failed. */
        UNHEALTHY
    }

    /**
     * Callback for health state transitions.
     */
    @FunctionalInterface
    public interface HealthStateListener {

        /**
         * Called when the backend health state changes.
         *
         * @param previousState The state before the transition
         * @param newState      The state after the transition
         * @param latencyMs     The measured round-trip latency in milliseconds
         *                      ({@code -1} if the check failed before latency could be measured)
         */
        void onStateChange(HealthState previousState, HealthState newState, long latencyMs);
    }

    private static final String HEALTH_CHECK_KEY_PREFIX = "/expressgateway/runtime-healthcheck/";
    private static final long DEFAULT_INTERVAL_MS = 10_000;
    private static final long DEFAULT_MAX_LATENCY_MS = 500;

    private final KVStore store;
    private final long intervalMs;
    private final long maxLatencyMs;
    private final AtomicReference<HealthState> state;
    private final CopyOnWriteArrayList<HealthStateListener> listeners;
    private final ScheduledExecutorService scheduler;
    private volatile ScheduledFuture<?> scheduledTask;
    private volatile boolean closed;

    /**
     * Creates a new health monitor with default settings (10s interval, 500ms max latency).
     *
     * @param store The KV store to monitor
     */
    public BackendHealthMonitor(KVStore store) {
        this(store, DEFAULT_INTERVAL_MS, DEFAULT_MAX_LATENCY_MS);
    }

    /**
     * Creates a new health monitor with custom interval and latency threshold.
     *
     * @param store        The KV store to monitor
     * @param intervalMs   The interval between health checks in milliseconds (must be > 0)
     * @param maxLatencyMs The maximum acceptable round-trip latency in milliseconds (must be > 0)
     * @throws IllegalArgumentException if intervalMs or maxLatencyMs is not positive
     */
    public BackendHealthMonitor(KVStore store, long intervalMs, long maxLatencyMs) {
        this.store = Objects.requireNonNull(store, "store");
        if (intervalMs <= 0) {
            throw new IllegalArgumentException("intervalMs must be > 0, got: " + intervalMs);
        }
        if (maxLatencyMs <= 0) {
            throw new IllegalArgumentException("maxLatencyMs must be > 0, got: " + maxLatencyMs);
        }
        this.intervalMs = intervalMs;
        this.maxLatencyMs = maxLatencyMs;
        this.state = new AtomicReference<>(HealthState.HEALTHY);
        this.listeners = new CopyOnWriteArrayList<>();

        ThreadFactory threadFactory = new ThreadFactory() {
            private final AtomicInteger counter = new AtomicInteger(0);

            @Override
            public Thread newThread(Runnable r) {
                Thread t = new Thread(r, "kvstore-health-monitor-" + counter.getAndIncrement());
                t.setDaemon(true);
                return t;
            }
        };
        this.scheduler = Executors.newSingleThreadScheduledExecutor(threadFactory);
    }

    /**
     * Starts the periodic health check. Safe to call multiple times; subsequent calls are no-ops
     * if the monitor is already running.
     */
    public void start() {
        if (closed) {
            throw new IllegalStateException("BackendHealthMonitor is already closed");
        }
        if (scheduledTask != null) {
            return; // Already running
        }
        scheduledTask = scheduler.scheduleWithFixedDelay(
                this::runCheck,
                0,
                intervalMs,
                TimeUnit.MILLISECONDS
        );
        log.info("BackendHealthMonitor started with interval={}ms, maxLatency={}ms", intervalMs, maxLatencyMs);
    }

    /**
     * Returns {@code true} if the backend is currently in a {@link HealthState#HEALTHY} state.
     */
    public boolean isHealthy() {
        return state.get() == HealthState.HEALTHY;
    }

    /**
     * Returns the current health state of the backend.
     */
    public HealthState currentState() {
        return state.get();
    }

    /**
     * Registers a listener for health state transitions.
     *
     * @param listener The listener to add
     */
    public void addListener(HealthStateListener listener) {
        Objects.requireNonNull(listener, "listener");
        listeners.add(listener);
    }

    /**
     * Removes a previously registered listener.
     *
     * @param listener The listener to remove
     * @return {@code true} if the listener was found and removed
     */
    public boolean removeListener(HealthStateListener listener) {
        return listeners.remove(listener);
    }

    @Override
    public void close() {
        if (closed) {
            return;
        }
        closed = true;

        ScheduledFuture<?> task = scheduledTask;
        if (task != null) {
            task.cancel(false);
        }

        scheduler.shutdown();
        try {
            if (!scheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                scheduler.shutdownNow();
            }
        } catch (InterruptedException e) {
            scheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }

        log.info("BackendHealthMonitor closed");
    }

    // ---- Internal ----

    private void runCheck() {
        String sentinelKey = HEALTH_CHECK_KEY_PREFIX + Thread.currentThread().threadId();
        String sentinelValue = "hc-" + System.nanoTime();
        byte[] valueBytes = sentinelValue.getBytes(StandardCharsets.UTF_8);

        long latencyMs = -1;
        HealthState newState;

        try {
            // Write + read cycle to measure connectivity and latency
            long startNanos = System.nanoTime();
            store.put(sentinelKey, valueBytes);

            Optional<KVEntry> entry = store.get(sentinelKey);
            latencyMs = (System.nanoTime() - startNanos) / 1_000_000;

            if (entry.isEmpty()) {
                log.warn("Runtime health check: sentinel key was written but could not be read back");
                newState = HealthState.UNHEALTHY;
            } else {
                String readValue = new String(entry.get().value(), StandardCharsets.UTF_8);
                if (!sentinelValue.equals(readValue)) {
                    log.warn("Runtime health check: sentinel value mismatch. Expected '{}', got '{}'",
                            sentinelValue, readValue);
                    newState = HealthState.UNHEALTHY;
                } else if (latencyMs > maxLatencyMs) {
                    log.debug("Runtime health check: latency {}ms exceeds max {}ms", latencyMs, maxLatencyMs);
                    newState = HealthState.DEGRADED;
                } else {
                    newState = HealthState.HEALTHY;
                }
            }

            // Best-effort cleanup
            cleanupSentinel(sentinelKey);

        } catch (KVStoreException e) {
            log.warn("Runtime health check failed: {}", e.getMessage());
            newState = HealthState.UNHEALTHY;
            cleanupSentinel(sentinelKey);
        } catch (Exception e) {
            log.warn("Runtime health check failed with unexpected error", e);
            newState = HealthState.UNHEALTHY;
            cleanupSentinel(sentinelKey);
        }

        // Transition state and notify listeners if changed
        HealthState previousState = state.getAndSet(newState);
        if (previousState != newState) {
            log.info("Backend health state changed: {} -> {} (latency={}ms)", previousState, newState, latencyMs);
            fireListeners(previousState, newState, latencyMs);
        }
    }

    private void fireListeners(HealthState previousState, HealthState newState, long latencyMs) {
        for (HealthStateListener listener : listeners) {
            try {
                listener.onStateChange(previousState, newState, latencyMs);
            } catch (Exception e) {
                log.error("Error notifying health state listener", e);
            }
        }
    }

    private void cleanupSentinel(String sentinelKey) {
        try {
            store.delete(sentinelKey);
        } catch (KVStoreException e) {
            log.debug("Failed to clean up runtime health check sentinel key {} (best-effort): {}",
                    sentinelKey, e.getMessage());
        }
    }
}
