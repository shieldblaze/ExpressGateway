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

import com.google.common.collect.EvictingQueue;
import com.shieldblaze.expressgateway.common.utils.MathUtil;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.time.Instant;
import java.util.Collections;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Health Check for checking health of remote host.
 *
 * <p>Supports configurable rise/fall thresholds (HAProxy-style), health result
 * caching, and exponential backoff for consecutive failures.</p>
 */
@SuppressWarnings("UnstableApiUsage")
public abstract class HealthCheck implements Runnable {

    protected final InetSocketAddress socketAddress;
    private final EvictingQueue<Boolean> queue;
    protected final int timeout;

    /**
     * Rise/fall thresholds for state transition hysteresis.
     */
    private final int consecutiveSuccessesForHealthy;
    private final int consecutiveFailuresForUnhealthy;
    private int consecutiveSuccesses;
    private int consecutiveFailures;

    /**
     * Cached health result to avoid recomputation on every query.
     */
    private final AtomicReference<CachedHealth> cachedHealth = new AtomicReference<>(
            new CachedHealth(Health.UNKNOWN, Instant.EPOCH));
    private final Duration cacheTtl;

    /**
     * Exponential backoff state for consecutive failures.
     */
    private final long baseBackoffMs;
    private final long maxBackoffMs;
    private int currentBackoffMultiplier;

    /**
     * Health check result record for caching.
     */
    record CachedHealth(Health health, Instant timestamp) {}

    /**
     * Health check result record for external consumption.
     */
    public record HealthResult(Health health, Duration latency, Instant checkedAt) {}

    /**
     * Create a new {@link HealthCheck} Instance with default settings.
     *
     * @param socketAddress {@link InetSocketAddress} of remote host
     * @param timeout       Timeout for health check
     */
    protected HealthCheck(InetSocketAddress socketAddress, Duration timeout) {
        this(socketAddress, timeout, 100);
    }

    /**
     * Create a new {@link HealthCheck} Instance with configurable samples.
     *
     * @param socketAddress {@link InetSocketAddress} of remote host
     * @param timeout       Timeout for health check
     * @param samples       Number of samples for evaluating health
     */
    protected HealthCheck(InetSocketAddress socketAddress, Duration timeout, int samples) {
        this(socketAddress, timeout, samples, 1, 1, Duration.ofSeconds(5),
                1000L, 60_000L);
    }

    /**
     * Create a new {@link HealthCheck} Instance with full configuration.
     *
     * @param socketAddress                    Remote host address
     * @param timeout                          Check timeout
     * @param samples                          Sample window size
     * @param consecutiveSuccessesForHealthy   Rise threshold
     * @param consecutiveFailuresForUnhealthy  Fall threshold
     * @param cacheTtl                         How long to cache health results
     * @param baseBackoffMs                    Base backoff in ms for exponential backoff
     * @param maxBackoffMs                     Maximum backoff in ms
     */
    protected HealthCheck(InetSocketAddress socketAddress, Duration timeout, int samples,
                          int consecutiveSuccessesForHealthy, int consecutiveFailuresForUnhealthy,
                          Duration cacheTtl, long baseBackoffMs, long maxBackoffMs) {
        this.socketAddress = socketAddress;
        this.timeout = (int) timeout.toMillis();
        this.queue = EvictingQueue.create(samples);

        if (consecutiveSuccessesForHealthy < 1) {
            throw new IllegalArgumentException(
                    "consecutiveSuccessesForHealthy must be >= 1, got: " + consecutiveSuccessesForHealthy);
        }
        if (consecutiveFailuresForUnhealthy < 1) {
            throw new IllegalArgumentException(
                    "consecutiveFailuresForUnhealthy must be >= 1, got: " + consecutiveFailuresForUnhealthy);
        }
        this.consecutiveSuccessesForHealthy = consecutiveSuccessesForHealthy;
        this.consecutiveFailuresForUnhealthy = consecutiveFailuresForUnhealthy;
        this.cacheTtl = cacheTtl != null ? cacheTtl : Duration.ofSeconds(5);
        this.baseBackoffMs = Math.max(0, baseBackoffMs);
        this.maxBackoffMs = Math.max(this.baseBackoffMs, maxBackoffMs);
        this.currentBackoffMultiplier = 0;
    }

    /**
     * If Health Check was successful, call this method.
     */
    protected void markSuccess() {
        synchronized (queue) {
            consecutiveSuccesses++;
            consecutiveFailures = 0;
            currentBackoffMultiplier = 0; // Reset backoff on success

            if (consecutiveSuccesses >= consecutiveSuccessesForHealthy) {
                queue.add(true);
            }
            invalidateCache();
        }
    }

    /**
     * If Health Check was unsuccessful, call this method.
     */
    protected void markFailure() {
        synchronized (queue) {
            consecutiveFailures++;
            consecutiveSuccesses = 0;

            // Exponential backoff: increase multiplier on each consecutive failure
            if (currentBackoffMultiplier < 20) { // cap to prevent overflow
                currentBackoffMultiplier++;
            }

            if (consecutiveFailures >= consecutiveFailuresForUnhealthy) {
                queue.add(false);
            }
            invalidateCache();
        }
    }

    private void invalidateCache() {
        cachedHealth.set(new CachedHealth(computeHealth(), Instant.now()));
    }

    private Health computeHealth() {
        if (queue.isEmpty()) {
            return Health.UNKNOWN;
        }
        double percentage = MathUtil.percentage(Collections.frequency(queue, true), queue.size());
        if (percentage >= 95) {
            return Health.GOOD;
        } else if (percentage >= 75) {
            return Health.MEDIUM;
        } else {
            return Health.BAD;
        }
    }

    /**
     * Get {@link Health} of Remote Host.
     * Returns a cached result if the cache TTL has not expired.
     */
    public Health health() {
        CachedHealth cached = cachedHealth.get();
        if (Duration.between(cached.timestamp(), Instant.now()).compareTo(cacheTtl) < 0) {
            return cached.health();
        }
        synchronized (queue) {
            Health h = computeHealth();
            cachedHealth.set(new CachedHealth(h, Instant.now()));
            return h;
        }
    }

    /**
     * Get the current backoff delay in milliseconds.
     * Returns 0 when the host is healthy (no consecutive failures).
     * Uses exponential backoff: baseBackoffMs * 2^(failures-1), capped at maxBackoffMs.
     */
    public long currentBackoffMs() {
        if (currentBackoffMultiplier <= 0) {
            return 0;
        }
        long backoff = baseBackoffMs * (1L << Math.min(currentBackoffMultiplier - 1, 30));
        return Math.min(backoff, maxBackoffMs);
    }

    /**
     * Returns the remote address being checked.
     */
    public InetSocketAddress socketAddress() {
        return socketAddress;
    }

    /**
     * Returns the configured rise threshold.
     */
    public int consecutiveSuccessesForHealthy() {
        return consecutiveSuccessesForHealthy;
    }

    /**
     * Returns the configured fall threshold.
     */
    public int consecutiveFailuresForUnhealthy() {
        return consecutiveFailuresForUnhealthy;
    }

    /**
     * Returns the current number of consecutive failures.
     */
    public int currentConsecutiveFailures() {
        return consecutiveFailures;
    }
}
