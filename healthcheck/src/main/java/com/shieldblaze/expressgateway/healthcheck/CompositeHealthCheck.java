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

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

/**
 * Composite health check that combines multiple individual health checks.
 *
 * <p>All component checks must pass for the composite to mark success.
 * If any check fails, the composite marks failure. Component checks
 * run concurrently using virtual threads for efficient I/O multiplexing.</p>
 *
 * <p>This enables patterns like "TCP connect + HTTP health endpoint" to
 * ensure both connectivity and application health.</p>
 */
public final class CompositeHealthCheck extends HealthCheck {

    private static final Logger logger = LogManager.getLogger(CompositeHealthCheck.class);

    private final List<HealthCheck> components;
    private final long perCheckTimeoutMs;

    /**
     * Create a composite health check.
     *
     * @param socketAddress Main backend address (for reporting)
     * @param timeout       Overall timeout for the composite check
     * @param samples       Sample window size
     * @param components    List of individual health checks to combine
     */
    public CompositeHealthCheck(InetSocketAddress socketAddress, Duration timeout,
                                int samples, List<HealthCheck> components) {
        super(socketAddress, timeout, samples);
        if (components == null || components.isEmpty()) {
            throw new IllegalArgumentException("components must not be empty");
        }
        this.components = List.copyOf(components);
        this.perCheckTimeoutMs = timeout.toMillis();
    }

    /**
     * Create a composite health check with rise/fall thresholds.
     */
    public CompositeHealthCheck(InetSocketAddress socketAddress, Duration timeout,
                                int samples, int rise, int fall,
                                List<HealthCheck> components) {
        super(socketAddress, timeout, samples, rise, fall,
                Duration.ofSeconds(5), 1000L, 60_000L);
        if (components == null || components.isEmpty()) {
            throw new IllegalArgumentException("components must not be empty");
        }
        this.components = List.copyOf(components);
        this.perCheckTimeoutMs = timeout.toMillis();
    }

    @Override
    public void run() {
        // Use virtual threads for concurrent health check execution
        try (ExecutorService executor = Executors.newVirtualThreadPerTaskExecutor()) {
            List<Future<?>> futures = new ArrayList<>();
            for (HealthCheck check : components) {
                futures.add(executor.submit((Runnable) check));
            }

            boolean allPassed = true;
            for (int i = 0; i < futures.size(); i++) {
                try {
                    futures.get(i).get(perCheckTimeoutMs, TimeUnit.MILLISECONDS);
                    // Check if the individual component reported healthy
                    Health componentHealth = components.get(i).health();
                    if (componentHealth == Health.BAD || componentHealth == Health.UNKNOWN) {
                        allPassed = false;
                    }
                } catch (Exception e) {
                    logger.debug("Composite health check component {} failed: {}",
                            components.get(i).socketAddress(), e.getMessage());
                    allPassed = false;
                }
            }

            if (allPassed) {
                markSuccess();
            } else {
                markFailure();
            }
        }
    }

    /**
     * Returns the list of component health checks.
     */
    public List<HealthCheck> components() {
        return components;
    }
}
