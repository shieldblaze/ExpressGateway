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

import com.shieldblaze.expressgateway.config.GatewayConfig;
import com.shieldblaze.expressgateway.coordination.CoordinationProvider;
import lombok.extern.log4j.Log4j2;

import java.util.Objects;
import java.util.Optional;

/**
 * Composition root for the gateway process. Replaces the old singleton pattern
 * with an explicit, non-static instance that holds all top-level gateway state.
 *
 * <p>There is exactly one {@code GatewayRuntime} per gateway process, but it is
 * NOT a singleton -- it is created by the bootstrapping code and passed to
 * components that need it. This makes the dependency explicit and testable.</p>
 *
 * <h2>Usage</h2>
 * <pre>{@code
 * GatewayConfig config = ConfigLoader.load(configPath);
 * GatewayRuntime runtime = new GatewayRuntime(config);
 * runtime.registerParticipant(myHttpServer);
 * runtime.start();
 * // ... gateway is running ...
 * runtime.stop();
 * }</pre>
 */
@Log4j2
public final class GatewayRuntime {

    private final GatewayConfig config;
    private final LifecycleManager lifecycleManager;
    private final ShutdownManager shutdownManager;
    private volatile CoordinationProvider coordinationProvider;

    /**
     * Creates a new gateway runtime with the given configuration.
     *
     * @param config the validated gateway configuration
     */
    public GatewayRuntime(GatewayConfig config) {
        this.config = Objects.requireNonNull(config, "config must not be null");
        this.lifecycleManager = new LifecycleManager(config);
        this.shutdownManager = new ShutdownManager(lifecycleManager);
    }

    /**
     * Returns the gateway configuration.
     *
     * @return the immutable config (never null)
     */
    public GatewayConfig config() {
        return config;
    }

    /**
     * Returns the lifecycle manager.
     *
     * @return the lifecycle manager (never null)
     */
    public LifecycleManager lifecycleManager() {
        return lifecycleManager;
    }

    /**
     * Returns the shutdown manager.
     *
     * @return the shutdown manager (never null)
     */
    public ShutdownManager shutdownManager() {
        return shutdownManager;
    }

    /**
     * Returns the coordination provider, if one has been set.
     * In STANDALONE mode, this will be empty.
     *
     * @return an Optional containing the coordination provider, or empty
     */
    public Optional<CoordinationProvider> coordinationProvider() {
        return Optional.ofNullable(coordinationProvider);
    }

    /**
     * Sets the coordination provider. Typically called during the CONNECT phase
     * by the coordination bootstrap code.
     *
     * @param provider the coordination provider
     */
    public void setCoordinationProvider(CoordinationProvider provider) {
        this.coordinationProvider = provider;
    }

    /**
     * Registers a lifecycle participant with the lifecycle manager.
     *
     * @param participant the participant to register
     */
    public void registerParticipant(LifecycleParticipant participant) {
        lifecycleManager.register(participant);
    }

    /**
     * Starts the gateway. Executes the full lifecycle startup sequence
     * and registers the JVM shutdown hook.
     *
     * @throws LifecycleException if startup fails
     */
    public void start() throws LifecycleException {
        log.info("Starting ShieldBlaze ExpressGateway (clusterId={}, mode={})",
                config.getClusterId(), config.getRunningMode());
        lifecycleManager.startup();
        shutdownManager.registerShutdownHook();
        log.info("ShieldBlaze ExpressGateway is running");
    }

    /**
     * Stops the gateway. Executes the full lifecycle shutdown sequence.
     */
    public void stop() {
        log.info("Stopping ShieldBlaze ExpressGateway");
        shutdownManager.initiateShutdown();

        // Close coordination provider if present
        if (coordinationProvider != null) {
            try {
                coordinationProvider.close();
                log.info("Coordination provider closed");
            } catch (Exception e) {
                log.warn("Error closing coordination provider", e);
            }
        }

        log.info("ShieldBlaze ExpressGateway stopped");
    }

    /**
     * Blocks the calling thread until the gateway shuts down.
     * Useful for main threads.
     *
     * @return true if shutdown completed within timeout, false if timed out
     */
    public boolean awaitShutdown() {
        return shutdownManager.awaitShutdown();
    }

    /**
     * Returns true if the gateway is in the RUNNING phase.
     */
    public boolean isRunning() {
        return lifecycleManager.isRunning();
    }
}
