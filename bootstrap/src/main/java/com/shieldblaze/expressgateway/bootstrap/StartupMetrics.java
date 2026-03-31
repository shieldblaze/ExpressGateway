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
package com.shieldblaze.expressgateway.bootstrap;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentMap;

/**
 * Tracks timing metrics during application startup. Records the wall-clock time
 * for each initialization component and the total time-to-ready.
 *
 * <p>Metrics are logged at INFO level and can be queried programmatically for
 * health check endpoints or monitoring dashboards.</p>
 */
public final class StartupMetrics {

    private static final Logger logger = LogManager.getLogger(StartupMetrics.class);

    private final ConcurrentMap<String, Duration> componentTimes = new ConcurrentHashMap<>();
    private volatile long startTimeNanos;
    private volatile long readyTimeNanos;

    /**
     * Mark the start of the overall startup process.
     */
    public void markStartupBegin() {
        startTimeNanos = System.nanoTime();
    }

    /**
     * Mark the end of the overall startup process (gateway is ready to serve traffic).
     */
    public void markStartupComplete() {
        readyTimeNanos = System.nanoTime();
        Duration total = timeToReady();
        logger.info("Startup complete. Time-to-ready: {}ms", total.toMillis());
        componentTimes.forEach((name, duration) ->
                logger.info("  Component '{}': {}ms", name, duration.toMillis()));
    }

    /**
     * Time a component initialization and record its duration.
     *
     * @param componentName the name of the component being initialized
     * @param task          the initialization task to time
     * @throws Exception if the task throws
     */
    public void timeComponent(String componentName, ThrowingRunnable task) throws Exception {
        long start = System.nanoTime();
        try {
            task.run();
            Duration elapsed = Duration.ofNanos(System.nanoTime() - start);
            componentTimes.put(componentName, elapsed);
            logger.info("Component '{}' initialized in {}ms", componentName, elapsed.toMillis());
        } catch (Exception ex) {
            Duration elapsed = Duration.ofNanos(System.nanoTime() - start);
            componentTimes.put(componentName + " (FAILED)", elapsed);
            throw ex;
        }
    }

    /**
     * Return the total time from startup begin to ready.
     */
    public Duration timeToReady() {
        if (readyTimeNanos == 0 || startTimeNanos == 0) {
            return Duration.ZERO;
        }
        return Duration.ofNanos(readyTimeNanos - startTimeNanos);
    }

    /**
     * Return the recorded component initialization times.
     */
    public Map<String, Duration> componentTimes() {
        return Map.copyOf(componentTimes);
    }

    /**
     * Return whether the startup has completed.
     */
    public boolean isReady() {
        return readyTimeNanos > 0;
    }

    /**
     * A runnable that may throw checked exceptions.
     */
    @FunctionalInterface
    public interface ThrowingRunnable {
        void run() throws Exception;
    }
}
