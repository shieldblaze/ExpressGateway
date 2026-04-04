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

import lombok.extern.slf4j.Slf4j;

import java.time.Duration;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Scheduler that runs health checks using virtual threads with per-backend
 * configurable intervals and exponential backoff for failing checks.
 *
 * <p>Health checks are executed on virtual threads (Java 21) for efficient
 * I/O multiplexing, allowing thousands of concurrent health checks without
 * platform thread exhaustion.</p>
 */
@Slf4j
public final class HealthCheckScheduler implements AutoCloseable {

    /**
     * A registered health check with its configured interval and last-run tracking.
     */
    public record ScheduledCheck(HealthCheck healthCheck, Duration interval) {}

    private static final class TrackedCheck {
        final ScheduledCheck scheduled;
        final AtomicLong lastRunNanos = new AtomicLong(0);

        TrackedCheck(ScheduledCheck scheduled) {
            this.scheduled = scheduled;
        }
    }

    private final CopyOnWriteArrayList<TrackedCheck> checks = new CopyOnWriteArrayList<>();
    private final ScheduledExecutorService scheduler;
    private final ExecutorService virtualThreadExecutor;
    private final AtomicBoolean running = new AtomicBoolean(false);

    public HealthCheckScheduler() {
        this.scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "hc-scheduler");
            t.setDaemon(true);
            return t;
        });
        this.virtualThreadExecutor = Executors.newVirtualThreadPerTaskExecutor();
    }

    /**
     * Register a health check with its interval.
     */
    public void register(HealthCheck healthCheck, Duration interval) {
        checks.add(new TrackedCheck(new ScheduledCheck(healthCheck, interval)));
    }

    /**
     * Unregister a health check.
     */
    public void unregister(HealthCheck healthCheck) {
        checks.removeIf(tc -> tc.scheduled.healthCheck() == healthCheck);
    }

    /**
     * Start the scheduler. Health checks run at their configured intervals,
     * with exponential backoff applied for failing checks.
     */
    public void start() {
        if (running.compareAndSet(false, true)) {
            scheduler.scheduleAtFixedRate(this::tick, 0, 1, TimeUnit.SECONDS);
            log.info("Health check scheduler started with {} checks", checks.size());
        }
    }

    /**
     * Stop the scheduler. Uses shutdownNow() to cancel pending tick tasks.
     */
    @Override
    public void close() {
        if (running.compareAndSet(true, false)) {
            scheduler.shutdownNow();
            virtualThreadExecutor.close();
            log.info("Health check scheduler stopped");
        }
    }

    /**
     * Returns the registered checks.
     */
    public List<ScheduledCheck> checks() {
        return checks.stream().map(tc -> tc.scheduled).toList();
    }

    /**
     * Returns the number of registered health checks.
     */
    public int size() {
        return checks.size();
    }

    /**
     * Returns true if the scheduler is running.
     */
    public boolean isRunning() {
        return running.get();
    }

    private void tick() {
        long now = System.nanoTime();
        for (TrackedCheck tc : checks) {
            long intervalNanos = tc.scheduled.interval().toNanos();
            long lastRun = tc.lastRunNanos.get();

            // Apply exponential backoff: use backoff delay instead of the normal interval
            // when the health check has consecutive failures.
            long backoffMs = tc.scheduled.healthCheck().currentBackoffMs();
            long effectiveIntervalNanos = backoffMs > 0
                    ? Math.max(intervalNanos, backoffMs * 1_000_000L)
                    : intervalNanos;

            if (now - lastRun >= effectiveIntervalNanos) {
                if (tc.lastRunNanos.compareAndSet(lastRun, now)) {
                    virtualThreadExecutor.submit(() -> {
                        try {
                            tc.scheduled.healthCheck().run();
                        } catch (Exception e) {
                            log.warn("Health check failed for {}: {}",
                                    tc.scheduled.healthCheck().socketAddress(), e.getMessage());
                        }
                    });
                }
            }
        }
    }
}
