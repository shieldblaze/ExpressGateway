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
package com.shieldblaze.expressgateway.configuration.distributed;

import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.state.ConnectionState;
import org.apache.curator.framework.state.ConnectionStateListener;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.util.Objects;

/**
 * ZooKeeper session event monitor for split-brain detection.
 *
 * <p>Implements {@link ConnectionStateListener} to track ZooKeeper connection health
 * and degrade gracefully:</p>
 * <ul>
 *   <li>{@code CONNECTED / RECONNECTED}: Normal operation</li>
 *   <li>{@code SUSPENDED}: Read-only mode (reject config changes)</li>
 *   <li>{@code LOST}: Full degradation (use last-known-good config)</li>
 * </ul>
 */
public final class SplitBrainDetector implements ConnectionStateListener, Closeable {

    private static final Logger logger = LogManager.getLogger(SplitBrainDetector.class);

    private final ConfigFallbackStore fallbackStore;
    private final java.util.function.Consumer<ConfigurationContext> fallbackCallback;

    private volatile ConnectionState currentState = ConnectionState.CONNECTED;

    /**
     * Creates a new {@link SplitBrainDetector}.
     *
     * @param fallbackStore    The {@link ConfigFallbackStore} to use for loading last-known-good config
     * @param fallbackCallback Callback invoked when falling back to LKG config on connection loss
     */
    public SplitBrainDetector(ConfigFallbackStore fallbackStore,
                              java.util.function.Consumer<ConfigurationContext> fallbackCallback) {
        this.fallbackStore = Objects.requireNonNull(fallbackStore, "fallbackStore");
        this.fallbackCallback = Objects.requireNonNull(fallbackCallback, "fallbackCallback");
    }

    /**
     * Register this detector as a connection state listener on the Curator instance.
     *
     * @throws Exception If an error occurs obtaining the Curator instance
     */
    public void start() throws Exception {
        CuratorFramework curator = Curator.getInstance();
        curator.getConnectionStateListenable().addListener(this);
        logger.info("Started SplitBrainDetector");
    }

    @Override
    public void stateChanged(CuratorFramework client, ConnectionState newState) {
        ConnectionState previousState = this.currentState;
        this.currentState = newState;

        logger.info("ZooKeeper connection state changed: {} -> {}", previousState, newState);

        switch (newState) {
            case CONNECTED -> {
                logger.info("ZooKeeper connection healthy, resuming normal operation");
            }
            case RECONNECTED -> {
                logger.info("ZooKeeper reconnected, resuming normal operation");
            }
            case SUSPENDED -> {
                logger.warn("ZooKeeper connection SUSPENDED, entering read-only mode — config changes will be rejected");
            }
            case LOST -> {
                logger.error("ZooKeeper connection LOST, entering full degradation mode — using last-known-good config");
                fallbackStore.loadLastKnownGood().ifPresent(lkg -> {
                    logger.info("Applying last-known-good configuration");
                    fallbackCallback.accept(lkg);
                });
            }
            case READ_ONLY -> {
                logger.warn("ZooKeeper connection in READ_ONLY mode, config changes will be rejected");
            }
            default -> {
                logger.warn("ZooKeeper connection entered unrecognized state: {}", newState);
            }
        }
    }

    /**
     * Check if the ZooKeeper connection is healthy (CONNECTED or RECONNECTED).
     *
     * @return {@code true} if the connection is healthy
     */
    public boolean isHealthy() {
        ConnectionState state = this.currentState;
        return state == ConnectionState.CONNECTED || state == ConnectionState.RECONNECTED;
    }

    /**
     * Check if the system is in read-only mode (SUSPENDED or READ_ONLY).
     * In this mode, configuration changes must be rejected.
     *
     * @return {@code true} if the system is in read-only mode
     */
    public boolean isReadOnly() {
        ConnectionState state = this.currentState;
        return state == ConnectionState.SUSPENDED || state == ConnectionState.READ_ONLY || state == ConnectionState.LOST;
    }

    @Override
    public void close() {
        logger.info("Closed SplitBrainDetector");
    }
}
