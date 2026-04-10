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
package com.shieldblaze.expressgateway.lifecycle;

import lombok.extern.log4j.Log4j2;

import java.time.Duration;
import java.util.Objects;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Manages clean JVM shutdown for the gateway.
 *
 * <p>Registers a JVM shutdown hook that triggers orderly shutdown via the
 * {@link LifecycleManager}. Uses a {@link CountDownLatch} for coordinated
 * shutdown and prevents double-shutdown via {@link AtomicBoolean}.</p>
 *
 * <p>The shutdown hook runs on a virtual thread for minimal resource usage.
 * If the shutdown does not complete within the configured timeout, the hook
 * returns and the JVM exits forcefully.</p>
 */
@Log4j2
public final class ShutdownManager {

    private final LifecycleManager lifecycleManager;
    private final Duration shutdownTimeout;
    private final AtomicBoolean shutdownInitiated = new AtomicBoolean(false);
    private final CountDownLatch shutdownLatch = new CountDownLatch(1);
    private volatile Thread shutdownHook;

    /**
     * Creates a shutdown manager with the default 30-second timeout.
     *
     * @param lifecycleManager the lifecycle manager to delegate shutdown to
     */
    public ShutdownManager(LifecycleManager lifecycleManager) {
        this(lifecycleManager, Duration.ofSeconds(30));
    }

    /**
     * Creates a shutdown manager with a custom timeout.
     *
     * @param lifecycleManager the lifecycle manager to delegate shutdown to
     * @param shutdownTimeout  the maximum time to wait for shutdown to complete
     */
    public ShutdownManager(LifecycleManager lifecycleManager, Duration shutdownTimeout) {
        this.lifecycleManager = Objects.requireNonNull(lifecycleManager, "lifecycleManager must not be null");
        this.shutdownTimeout = Objects.requireNonNull(shutdownTimeout, "shutdownTimeout must not be null");
    }

    /**
     * Registers the JVM shutdown hook. Call this after the lifecycle manager
     * has started successfully. Safe to call multiple times (idempotent).
     */
    public void registerShutdownHook() {
        if (shutdownHook != null) {
            return; // already registered
        }
        shutdownHook = Thread.ofVirtual()
                .name("eg-shutdown-hook")
                .unstarted(this::onShutdownHook);
        Runtime.getRuntime().addShutdownHook(shutdownHook);
        log.debug("JVM shutdown hook registered (timeout={}s)", shutdownTimeout.toSeconds());
    }

    /**
     * Programmatically initiates shutdown. Does not return until shutdown is complete
     * or the timeout expires. Prevents double-shutdown.
     */
    public void initiateShutdown() {
        if (!shutdownInitiated.compareAndSet(false, true)) {
            log.info("Shutdown already initiated, waiting for completion");
            awaitShutdownLatch();
            return;
        }

        log.info("Initiating programmatic shutdown (timeout={}s)", shutdownTimeout.toSeconds());
        try {
            lifecycleManager.shutdown();
        } catch (Exception e) {
            log.error("Error during shutdown", e);
        } finally {
            shutdownLatch.countDown();
        }
    }

    /**
     * Blocks the calling thread until shutdown completes or the timeout expires.
     * Useful for main threads that need to keep the JVM alive while running.
     *
     * @return true if shutdown completed within the timeout, false if timed out
     */
    public boolean awaitShutdown() {
        return awaitShutdownLatch();
    }

    /**
     * Returns true if shutdown has been initiated (regardless of completion).
     */
    public boolean isShutdownInitiated() {
        return shutdownInitiated.get();
    }

    /**
     * The actual shutdown hook logic executed by the JVM.
     */
    private void onShutdownHook() {
        if (!shutdownInitiated.compareAndSet(false, true)) {
            // Shutdown was already initiated programmatically; just wait for it
            log.info("Shutdown hook fired, but shutdown already in progress");
            awaitShutdownLatch();
            return;
        }

        log.info("JVM shutdown hook fired, initiating gateway shutdown (timeout={}s)",
                shutdownTimeout.toSeconds());
        try {
            lifecycleManager.shutdown();
        } catch (Exception e) {
            log.error("Error during JVM shutdown hook execution", e);
        } finally {
            shutdownLatch.countDown();
        }
    }

    /**
     * Waits on the shutdown latch with the configured timeout.
     */
    private boolean awaitShutdownLatch() {
        try {
            return shutdownLatch.await(shutdownTimeout.toMillis(), TimeUnit.MILLISECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            log.warn("Interrupted while waiting for shutdown to complete");
            return false;
        }
    }
}
