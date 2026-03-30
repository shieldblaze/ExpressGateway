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

import java.nio.charset.StandardCharsets;
import java.util.Objects;
import java.util.Optional;

/**
 * Startup safety validator that checks KV store backend health before
 * the control plane starts accepting traffic.
 *
 * <p>The control plane MUST refuse to start if any check fails after all
 * configured retries. This prevents the gateway from running in a degraded
 * state where config reads/writes silently fail.</p>
 *
 * <p>Checks performed on each attempt:</p>
 * <ol>
 *   <li><b>Connectivity</b> -- can we reach the backend?</li>
 *   <li><b>Read/Write</b> -- can we perform a basic put/get/delete cycle?</li>
 *   <li><b>Latency</b> -- is the round-trip time within acceptable bounds?</li>
 * </ol>
 *
 * <p>Retries up to {@link StorageConfiguration#startupHealthCheckRetries()} times
 * with exponential backoff (1s, 2s, 4s, ...).</p>
 */
@Log4j2
public final class BackendHealthChecker {

    private static final String HEALTH_CHECK_KEY_PREFIX = "/expressgateway/healthcheck/";

    private BackendHealthChecker() {
        // No instantiation -- all static
    }

    /**
     * Performs startup health checks on the KV store.
     *
     * <p>Writes a sentinel key, reads it back and verifies the value matches,
     * measures round-trip latency, and cleans up the sentinel. If any step fails,
     * the attempt is retried with exponential backoff. If all retries are exhausted,
     * a {@link KVStoreException} is thrown to prevent the control plane from starting.</p>
     *
     * @param store  the KV store to check; must not be null
     * @param config the storage configuration containing health check parameters; must not be null
     * @throws KVStoreException if any check fails after all retries
     */
    public static void check(KVStore store, StorageConfiguration config) throws KVStoreException {
        Objects.requireNonNull(store, "store");
        Objects.requireNonNull(config, "config");

        int maxRetries = config.startupHealthCheckRetries();
        long maxLatencyMs = config.maxAcceptableLatencyMs();
        long backoffMs = 1000;

        KVStoreException lastException = null;

        for (int attempt = 1; attempt <= maxRetries; attempt++) {
            log.info("Startup health check attempt {}/{}", attempt, maxRetries);

            try {
                runHealthCheck(store, maxLatencyMs);
                log.info("Startup health check passed on attempt {}/{}", attempt, maxRetries);
                return;
            } catch (KVStoreException e) {
                lastException = e;
                log.warn("Startup health check failed on attempt {}/{}: {}", attempt, maxRetries, e.getMessage());

                if (attempt < maxRetries) {
                    log.info("Retrying in {}ms (exponential backoff)", backoffMs);
                    try {
                        Thread.sleep(backoffMs);
                    } catch (InterruptedException ie) {
                        Thread.currentThread().interrupt();
                        throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                                "Health check interrupted during backoff", ie);
                    }
                    backoffMs = Math.min(backoffMs * 2, config.startupHealthCheckTimeoutMs());
                }
            }
        }

        String detail = lastException != null ? lastException.getMessage() : "unknown";
        throw new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                "Startup health check failed after " + maxRetries + " attempts: " + detail,
                lastException);
    }

    /**
     * Executes a single health check cycle: put, get, verify, delete, and latency measurement.
     */
    private static void runHealthCheck(KVStore store, long maxLatencyMs) throws KVStoreException {
        String sentinelKey = HEALTH_CHECK_KEY_PREFIX + System.currentTimeMillis();
        String sentinelValue = "healthcheck-" + System.nanoTime();
        byte[] valueBytes = sentinelValue.getBytes(StandardCharsets.UTF_8);

        try {
            // 1. Write sentinel
            long startNanos = System.nanoTime();
            store.put(sentinelKey, valueBytes);
            long putLatencyMs = (System.nanoTime() - startNanos) / 1_000_000;
            log.debug("Health check PUT completed in {}ms for key {}", putLatencyMs, sentinelKey);

            // 2. Read back and verify
            startNanos = System.nanoTime();
            Optional<KVEntry> entry = store.get(sentinelKey);
            long getLatencyMs = (System.nanoTime() - startNanos) / 1_000_000;
            log.debug("Health check GET completed in {}ms for key {}", getLatencyMs, sentinelKey);

            if (entry.isEmpty()) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Health check failed: sentinel key was written but could not be read back: " + sentinelKey);
            }

            String readValue = new String(entry.get().value(), StandardCharsets.UTF_8);
            if (!sentinelValue.equals(readValue)) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Health check failed: sentinel value mismatch. Expected '" + sentinelValue
                                + "', got '" + readValue + "'");
            }

            // 3. Check latency
            long roundTripMs = putLatencyMs + getLatencyMs;
            log.debug("Health check round-trip latency: {}ms (max acceptable: {}ms)", roundTripMs, maxLatencyMs);

            if (roundTripMs > maxLatencyMs) {
                throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Health check failed: round-trip latency " + roundTripMs
                                + "ms exceeds maximum acceptable latency " + maxLatencyMs + "ms");
            }

            // 4. Clean up sentinel key
            store.delete(sentinelKey);
            log.debug("Health check sentinel key deleted: {}", sentinelKey);

        } catch (KVStoreException e) {
            // Attempt cleanup even on failure
            cleanupSentinel(store, sentinelKey);
            throw e;
        }
    }

    /**
     * Best-effort cleanup of a sentinel key. Failures are logged but not propagated.
     */
    private static void cleanupSentinel(KVStore store, String sentinelKey) {
        try {
            store.delete(sentinelKey);
        } catch (KVStoreException e) {
            log.debug("Failed to clean up sentinel key {} (best-effort): {}", sentinelKey, e.getMessage());
        }
    }
}
