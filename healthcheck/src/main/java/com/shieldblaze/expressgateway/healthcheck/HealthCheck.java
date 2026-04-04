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

import com.shieldblaze.expressgateway.common.utils.MathUtil;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.time.Instant;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import java.util.concurrent.locks.ReentrantLock;

/**
 * Health Check for checking health of remote host.
 *
 * <p>Supports configurable rise/fall thresholds (HAProxy-style), health result
 * caching, and exponential backoff for consecutive failures.</p>
 *
 * <p>Thread safety: Internal state is protected by a {@link ReentrantLock} to
 * avoid contention from virtual-thread pinning that {@code synchronized} blocks
 * cause. The lock is only held during the brief state mutation in markSuccess/markFailure.
 * The health() read path uses an atomic cached value for lock-free reads.</p>
 */
public abstract class HealthCheck implements Runnable {

    protected final InetSocketAddress socketAddress;
    protected final int timeout;

    /**
     * Circular buffer for health check sample history (replaces Guava EvictingQueue).
     * Using a plain array with modular index avoids the Guava @Beta/unstable API warning
     * and eliminates the autoboxing overhead of EvictingQueue&lt;Boolean&gt;.
     */
    private final boolean[] samples;
    private final int sampleCapacity;
    private int sampleHead;
    private int sampleCount;

    private final ReentrantLock lock = new ReentrantLock();

    /**
     * Rise/fall thresholds for state transition hysteresis.
     */
    private final int consecutiveSuccessesForHealthy;
    private final int consecutiveFailuresForUnhealthy;
    private int consecutiveSuccesses;
    private final AtomicInteger consecutiveFailureCount = new AtomicInteger(0);

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
    private volatile int currentBackoffMultiplier;

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
     * @param sampleSize                       Sample window size
     * @param consecutiveSuccessesForHealthy   Rise threshold
     * @param consecutiveFailuresForUnhealthy  Fall threshold
     * @param cacheTtl                         How long to cache health results
     * @param baseBackoffMs                    Base backoff in ms for exponential backoff
     * @param maxBackoffMs                     Maximum backoff in ms
     */
    protected HealthCheck(InetSocketAddress socketAddress, Duration timeout, int sampleSize,
                          int consecutiveSuccessesForHealthy, int consecutiveFailuresForUnhealthy,
                          Duration cacheTtl, long baseBackoffMs, long maxBackoffMs) {
        this.socketAddress = socketAddress;
        this.timeout = (int) timeout.toMillis();

        if (sampleSize < 1) {
            throw new IllegalArgumentException("sampleSize must be >= 1, got: " + sampleSize);
        }
        this.sampleCapacity = sampleSize;
        this.samples = new boolean[sampleSize];
        this.sampleHead = 0;
        this.sampleCount = 0;

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
        lock.lock();
        try {
            consecutiveSuccesses++;
            consecutiveFailureCount.set(0);
            currentBackoffMultiplier = 0;

            if (consecutiveSuccesses >= consecutiveSuccessesForHealthy) {
                addSample(true);
            }
            invalidateCache();
        } finally {
            lock.unlock();
        }
    }

    /**
     * If Health Check was unsuccessful, call this method.
     */
    protected void markFailure() {
        lock.lock();
        try {
            int failures = consecutiveFailureCount.incrementAndGet();
            consecutiveSuccesses = 0;

            if (currentBackoffMultiplier < 20) {
                currentBackoffMultiplier++;
            }

            if (failures >= consecutiveFailuresForUnhealthy) {
                addSample(false);
            }
            invalidateCache();
        } finally {
            lock.unlock();
        }
    }

    private void addSample(boolean success) {
        samples[sampleHead] = success;
        sampleHead = (sampleHead + 1) % sampleCapacity;
        if (sampleCount < sampleCapacity) {
            sampleCount++;
        }
    }

    private void invalidateCache() {
        cachedHealth.set(new CachedHealth(computeHealth(), Instant.now()));
    }

    private Health computeHealth() {
        if (sampleCount == 0) {
            return Health.UNKNOWN;
        }
        int successCount = 0;
        for (int i = 0; i < sampleCount; i++) {
            if (samples[i]) {
                successCount++;
            }
        }
        double percentage = MathUtil.percentage(successCount, sampleCount);
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
     * Uses double-check locking: fast path reads the atomic cached value,
     * slow path acquires the lock and re-checks before recomputing.
     */
    public Health health() {
        CachedHealth cached = cachedHealth.get();
        if (Duration.between(cached.timestamp(), Instant.now()).compareTo(cacheTtl) < 0) {
            return cached.health();
        }
        lock.lock();
        try {
            // Double-check: another thread may have refreshed while we waited for the lock
            cached = cachedHealth.get();
            if (Duration.between(cached.timestamp(), Instant.now()).compareTo(cacheTtl) < 0) {
                return cached.health();
            }
            Health h = computeHealth();
            cachedHealth.set(new CachedHealth(h, Instant.now()));
            return h;
        } finally {
            lock.unlock();
        }
    }

    /**
     * Get the {@link InetSocketAddress} of the remote host being checked.
     */
    public InetSocketAddress socketAddress() {
        return socketAddress;
    }

    /**
     * Get the current backoff delay in milliseconds.
     * Returns 0 when the host is healthy (no consecutive failures).
     * Uses exponential backoff: baseBackoffMs * 2^(failures-1), capped at maxBackoffMs.
     */
    public long currentBackoffMs() {
        int multiplier = currentBackoffMultiplier;
        if (multiplier <= 0) {
            return 0;
        }
        long backoff = baseBackoffMs * (1L << Math.min(multiplier - 1, 30));
        return Math.min(backoff, maxBackoffMs);
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
        return consecutiveFailureCount.get();
    }
}
