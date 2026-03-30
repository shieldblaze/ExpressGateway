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

import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.common.zookeeper.ZNodePath;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;

/**
 * Leader election wrapper backed by a {@link ConfigStorageBackend} or an external
 * leadership source (delegating mode).
 *
 * <p>Leader election path (standalone mode):
 * {@code /ExpressGateway/{env}/{clusterId}/config/leader}</p>
 *
 * <p>D-3.1 fix: In delegating mode, this class wraps an external leadership supplier
 * (e.g., {@code ControlPlaneCluster.isLeader()}) so that both the control plane cluster
 * and the distributed configuration system use a SINGLE leader election path, preventing
 * split-brain where two different nodes believe they are the leader.</p>
 *
 * <p>Only the leader node is permitted to propose and apply configuration changes
 * in the distributed configuration system.</p>
 */
public final class ConfigLeaderElection implements Closeable {

    private static final Logger logger = LogManager.getLogger(ConfigLeaderElection.class);

    private static final String ROOT_PATH = "ExpressGateway";
    private static final String CONFIG_COMPONENT = "config";
    private static final String LEADER_COMPONENT = "leader";

    private final String clusterId;
    private final Environment environment;
    private final String participantId;
    private final ConfigStorageBackend storageBackend;

    // Delegating mode fields (D-3.1 fix)
    private final BooleanSupplier externalLeaderSupplier;
    private final List<Consumer<Boolean>> delegatingListeners;
    private final boolean delegating;

    private volatile ConfigStorageBackend.LeaderElection leaderElection;

    /**
     * Creates a new {@link ConfigLeaderElection} in standalone mode (runs its own election).
     *
     * @param clusterId      The cluster identifier
     * @param environment    The deployment environment
     * @param participantId  A unique identifier for this participant (e.g., node ID)
     * @param storageBackend The storage backend for leader election
     */
    public ConfigLeaderElection(String clusterId, Environment environment, String participantId,
                                ConfigStorageBackend storageBackend) {
        this.clusterId = Objects.requireNonNull(clusterId, "clusterId");
        this.environment = Objects.requireNonNull(environment, "environment");
        this.participantId = Objects.requireNonNull(participantId, "participantId");
        this.storageBackend = Objects.requireNonNull(storageBackend, "storageBackend");
        this.externalLeaderSupplier = null;
        this.delegatingListeners = null;
        this.delegating = false;
    }

    /**
     * Creates a new {@link ConfigLeaderElection} in delegating mode (D-3.1 fix).
     *
     * <p>Instead of running its own leader election, this instance delegates to an external
     * leadership source (e.g., {@code ControlPlaneCluster}). This ensures a single leader
     * election path for the entire system.</p>
     *
     * @param externalLeaderSupplier Supplies the current leadership state from the unified election
     */
    public ConfigLeaderElection(BooleanSupplier externalLeaderSupplier) {
        this.externalLeaderSupplier = Objects.requireNonNull(externalLeaderSupplier, "externalLeaderSupplier");
        this.delegatingListeners = new CopyOnWriteArrayList<>();
        this.delegating = true;
        this.clusterId = null;
        this.environment = null;
        this.participantId = null;
        this.storageBackend = null;
    }

    /**
     * Start participating in leader election.
     * In delegating mode, this is a no-op (the external election is already running).
     *
     * @throws Exception If an error occurs during initialization
     */
    public void start() throws Exception {
        if (delegating) {
            logger.info("ConfigLeaderElection in delegating mode — using external leader election (D-3.1 unified)");
            return;
        }
        String leaderPath = ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT, LEADER_COMPONENT).path();

        leaderElection = storageBackend.leaderElection(leaderPath, participantId);
        leaderElection.start();

        logger.info("Started leader election participation on path: {} with participant: {}", leaderPath, participantId);
    }

    /**
     * Check if this node is currently the leader.
     *
     * @return {@code true} if this node holds the leadership
     */
    public boolean isLeader() {
        if (delegating) {
            return externalLeaderSupplier.getAsBoolean();
        }
        ConfigStorageBackend.LeaderElection election = this.leaderElection;
        return election != null && election.isLeader();
    }

    /**
     * Add a listener for leadership state changes.
     *
     * @param listener Called with {@code true} when leadership is gained, {@code false} when lost
     */
    public void addListener(Consumer<Boolean> listener) {
        Objects.requireNonNull(listener, "listener");
        if (delegating) {
            delegatingListeners.add(listener);
            return;
        }
        ConfigStorageBackend.LeaderElection election = this.leaderElection;
        if (election != null) {
            election.addListener(listener);
        } else {
            throw new IllegalStateException("Leader election has not been started");
        }
    }

    /**
     * Notifies delegating listeners of a leadership change.
     * Called by the external election source (e.g., ControlPlaneCluster) when leadership changes.
     *
     * @param isLeader {@code true} if this node gained leadership, {@code false} if lost
     */
    public void notifyLeadershipChange(boolean isLeader) {
        if (!delegating || delegatingListeners == null) {
            return;
        }
        for (Consumer<Boolean> listener : delegatingListeners) {
            try {
                listener.accept(isLeader);
            } catch (Exception e) {
                logger.error("Error notifying delegating leadership listener", e);
            }
        }
    }

    @Override
    public void close() throws IOException {
        if (delegating) {
            if (delegatingListeners != null) {
                delegatingListeners.clear();
            }
            logger.info("Closed delegating ConfigLeaderElection");
            return;
        }
        ConfigStorageBackend.LeaderElection election = this.leaderElection;
        this.leaderElection = null; // Null out before closing so isLeader() returns false immediately
        if (election != null) {
            election.close();
            logger.info("Closed leader election for participant: {}", participantId);
        }
    }
}
