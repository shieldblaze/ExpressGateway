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

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.time.Duration;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;

/**
 * Top-level coordinator for the distributed configuration system.
 *
 * <p>Ties together leader election, versioned config storage, config watching,
 * split-brain detection, local fallback, quorum-based commit, audit logging,
 * crash recovery, and config validation into a single coherent API.</p>
 *
 * <p>Quorum-aware 2PC protocol for {@link #proposeConfig(ConfigurationContext)}:</p>
 * <ol>
 *   <li>If not leader, throw {@link IllegalStateException}</li>
 *   <li>Validate the config locally via {@link ConfigValidator}</li>
 *   <li>Write proposed config to a new version node</li>
 *   <li>Log PROPOSE to audit trail</li>
 *   <li>Leader ACKs the version immediately</li>
 *   <li>Wait for quorum ACKs from cluster nodes</li>
 *   <li>If quorum reached: update {@code /config/current}, log COMMIT, save to LKG,
 *       return {@link ConfigRolloutState#CONFIRMED}</li>
 *   <li>If quorum timeout: log REJECT, auto-rollback,
 *       return {@link ConfigRolloutState#TIMED_OUT}</li>
 * </ol>
 */
public final class DistributedConfigurationManager implements Closeable {

    private static final Logger logger = LogManager.getLogger(DistributedConfigurationManager.class);

    /**
     * Default quorum timeout. Can be overridden via {@link #setQuorumTimeout(Duration)}.
     */
    private static final Duration DEFAULT_QUORUM_TIMEOUT = Duration.ofSeconds(30);

    /**
     * Default assumed cluster size when no external source is available.
     * In production, this should be set to the actual number of registered nodes.
     */
    private static final int DEFAULT_CLUSTER_SIZE = 3;

    private final String clusterId;
    private final Environment environment;

    private final ConfigVersionStore versionStore;
    private final ConfigLeaderElection leaderElection;
    private final ConfigWatcher configWatcher;
    private final ConfigFallbackStore fallbackStore;
    private final SplitBrainDetector splitBrainDetector;
    private final ConfigQuorumManager quorumManager;
    private final ConfigAuditLog auditLog;

    private volatile ConfigurationContext currentConfig;
    private volatile boolean started;
    private volatile Duration quorumTimeout;
    private volatile int clusterSize;

    /**
     * Creates a new {@link DistributedConfigurationManager} using the default
     * Curator-backed storage backend.
     *
     * @param clusterId   The cluster identifier
     * @param environment The deployment environment
     */
    public DistributedConfigurationManager(String clusterId, Environment environment) {
        this(clusterId, environment, DEFAULT_CLUSTER_SIZE);
    }

    /**
     * Creates a new {@link DistributedConfigurationManager} using the default
     * Curator-backed storage backend with explicit cluster size.
     *
     * @param clusterId   The cluster identifier
     * @param environment The deployment environment
     * @param clusterSize The number of nodes in the cluster (for quorum calculation)
     */
    public DistributedConfigurationManager(String clusterId, Environment environment, int clusterSize) {
        this(clusterId, environment, clusterSize, new CuratorConfigStorageBackend());
    }

    /**
     * Creates a new {@link DistributedConfigurationManager} with an explicit storage backend.
     *
     * @param clusterId      The cluster identifier
     * @param environment    The deployment environment
     * @param clusterSize    The number of nodes in the cluster (for quorum calculation)
     * @param storageBackend The storage backend to use for all distributed operations
     */
    public DistributedConfigurationManager(String clusterId, Environment environment, int clusterSize,
                                           ConfigStorageBackend storageBackend) {
        this(clusterId, environment, clusterSize, storageBackend, null);
    }

    /**
     * Creates a new {@link DistributedConfigurationManager} with an external leader election supplier.
     *
     * <p>D-3.1 fix: When {@code externalLeaderSupplier} is provided, the config leader election
     * delegates to it instead of running its own election. This ensures the control plane cluster
     * and the distributed configuration system share a single, unified leader election path.</p>
     *
     * @param clusterId              The cluster identifier
     * @param environment            The deployment environment
     * @param clusterSize            The number of nodes in the cluster (for quorum calculation)
     * @param storageBackend         The storage backend to use for all distributed operations
     * @param externalLeaderSupplier External leadership supplier (from ControlPlaneCluster), or null for standalone
     */
    public DistributedConfigurationManager(String clusterId, Environment environment, int clusterSize,
                                           ConfigStorageBackend storageBackend,
                                           java.util.function.BooleanSupplier externalLeaderSupplier) {
        this.clusterId = Objects.requireNonNull(clusterId, "clusterId");
        this.environment = Objects.requireNonNull(environment, "environment");
        Objects.requireNonNull(storageBackend, "storageBackend");
        if (clusterSize < 1) {
            throw new IllegalArgumentException("clusterSize must be >= 1, got: " + clusterSize);
        }
        this.clusterSize = clusterSize;

        this.versionStore = new ConfigVersionStore(clusterId, environment, storageBackend);
        this.fallbackStore = new ConfigFallbackStore();

        String participantId = ExpressGateway.getInstance().ID();
        // D-3.1: Use delegating mode if external leader supplier is provided,
        // ensuring a single unified leader election for the entire system.
        if (externalLeaderSupplier != null) {
            this.leaderElection = new ConfigLeaderElection(externalLeaderSupplier);
        } else {
            this.leaderElection = new ConfigLeaderElection(clusterId, environment, participantId, storageBackend);
        }
        this.splitBrainDetector = new SplitBrainDetector(fallbackStore, this::onFallbackConfig);
        this.configWatcher = new ConfigWatcher(clusterId, environment, versionStore, this::onConfigChange, storageBackend);
        this.quorumManager = new ConfigQuorumManager(clusterId, environment, participantId, storageBackend);
        this.auditLog = new ConfigAuditLog(clusterId, environment, participantId, storageBackend);

        this.currentConfig = ConfigurationContext.DEFAULT;
        this.quorumTimeout = DEFAULT_QUORUM_TIMEOUT;
    }

    /**
     * Returns the leader election instance for external wiring (e.g., to notify
     * of leadership changes from the unified ControlPlaneCluster election).
     */
    public ConfigLeaderElection leaderElection() {
        return leaderElection;
    }

    /**
     * Start the distributed configuration manager.
     *
     * <p>Initializes leader election, config watcher, split-brain monitor, and runs
     * crash recovery if this node is the leader. Attempts to load the current configuration
     * from the backend, falling back to last-known-good on failure.</p>
     *
     * @throws Exception If an error occurs during startup
     */
    public void start() throws Exception {
        if (started) {
            throw new IllegalStateException("DistributedConfigurationManager is already started");
        }

        logger.info("Starting DistributedConfigurationManager for cluster: {}, environment: {}",
                clusterId, environment);

        // Start leader election
        leaderElection.start();

        // Start split-brain detector
        splitBrainDetector.start();

        // Try to load current config
        try {
            int currentVersion = versionStore.currentVersion();
            if (currentVersion > 0) {
                this.currentConfig = versionStore.readVersion(currentVersion);
                logger.info("Loaded current configuration version {} from backend", currentVersion);

                // Persist as LKG
                fallbackStore.saveLastKnownGood(this.currentConfig);
            } else {
                logger.info("No current configuration version found, using default");
            }
        } catch (Exception e) {
            logger.warn("Failed to load current configuration from backend, attempting LKG fallback", e);
            fallbackStore.loadLastKnownGood().ifPresent(lkg -> {
                this.currentConfig = lkg;
                logger.info("Loaded last-known-good configuration from fallback store");
            });
        }

        // Run crash recovery if this node is the leader
        if (leaderElection.isLeader()) {
            try {
                CrashRecoveryManager recoveryManager = new CrashRecoveryManager(
                        versionStore, quorumManager, auditLog, fallbackStore, clusterSize);
                recoveryManager.recoverIfNeeded();

                // Reload current config after recovery (pointer may have changed)
                try {
                    int recoveredVersion = versionStore.currentVersion();
                    if (recoveredVersion > 0) {
                        this.currentConfig = versionStore.readVersion(recoveredVersion);
                        fallbackStore.saveLastKnownGood(this.currentConfig);
                    }
                } catch (Exception e) {
                    logger.warn("Failed to reload configuration after crash recovery", e);
                }
            } catch (Exception e) {
                logger.error("Crash recovery failed, continuing with current config", e);
            }
        }

        // Start watching for config changes
        configWatcher.start();

        started = true;
        logger.info("DistributedConfigurationManager started successfully");
    }

    /**
     * Propose a new configuration to the cluster.
     *
     * <p>Only the leader can propose configuration changes. Uses quorum-aware 2PC protocol:
     * validates locally, writes to backend, waits for quorum ACKs, then commits or auto-rolls-back.</p>
     *
     * @param context The proposed {@link ConfigurationContext}
     * @return A {@link CompletableFuture} that completes with the final {@link ConfigRolloutState}
     * @throws IllegalStateException If this node is not the leader or the system is in read-only mode
     */
    public CompletableFuture<ConfigRolloutState> proposeConfig(ConfigurationContext context) {
        Objects.requireNonNull(context, "ConfigurationContext");

        return CompletableFuture.supplyAsync(() -> {
            // Capture volatile fields into locals at proposal start to ensure consistent
            // values throughout the entire proposal lifecycle (fixes MEDIUM-2.2: volatile
            // read could change mid-proposal if setClusterSize() is called concurrently).
            final int snapshotClusterSize = this.clusterSize;
            final Duration snapshotQuorumTimeout = this.quorumTimeout;

            try {
                // Step 1: Verify leadership
                if (!leaderElection.isLeader()) {
                    throw new IllegalStateException("Only the leader can propose configuration changes");
                }

                // Verify connection health
                if (splitBrainDetector.isReadOnly()) {
                    throw new IllegalStateException("Cannot propose configuration changes in read-only mode " +
                            "(backend connection is degraded)");
                }

                // Step 2: Validate configuration before writing
                List<String> validationErrors = ConfigValidator.validate(context);
                if (!validationErrors.isEmpty()) {
                    logger.error("Configuration validation failed: {}", validationErrors);
                    auditLog.log("pending", ConfigAuditLog.AuditAction.REJECT,
                            "validation-failed: " + validationErrors, currentVersionString());
                    throw new IllegalArgumentException("Configuration validation failed: " + validationErrors);
                }

                logger.info("Proposing new configuration (validation passed)");

                // Capture previous version for audit trail
                String previousVersionStr = currentVersionString();

                // Step 3: Write proposed config to new version
                int newVersion = versionStore.writeVersion(context);
                String newVersionStr = ConfigQuorumManager.formatVersion(newVersion);
                logger.info("Wrote proposed configuration as version {}", newVersionStr);

                // Step 4: Log PROPOSE to audit trail
                auditLog.log(newVersionStr, ConfigAuditLog.AuditAction.PROPOSE,
                        "operator-initiated", previousVersionStr);

                // Step 5: Leader ACKs its own version immediately
                quorumManager.acknowledgeConfig(newVersionStr);

                // Step 6: Wait for quorum ACKs
                int requiredAcks = ConfigQuorumManager.quorumSize(snapshotClusterSize);
                logger.info("Waiting for quorum: {}/{} ACKs required, timeout: {}",
                        requiredAcks, snapshotClusterSize, snapshotQuorumTimeout);

                boolean quorumReached = quorumManager.awaitQuorum(newVersionStr, requiredAcks, snapshotQuorumTimeout)
                        .join();

                if (quorumReached) {
                    // Step 7a: Quorum reached -- commit
                    versionStore.setCurrent(newVersion);
                    fallbackStore.saveLastKnownGood(context);
                    this.currentConfig = context;

                    auditLog.log(newVersionStr, ConfigAuditLog.AuditAction.COMMIT,
                            "quorum-reached", previousVersionStr);

                    logger.info("Configuration version {} committed successfully (quorum reached)", newVersionStr);
                    return ConfigRolloutState.CONFIRMED;

                } else {
                    // Step 7b: Quorum timeout -- auto-rollback
                    logger.warn("Quorum timeout for version {}, auto-rolling back", newVersionStr);

                    auditLog.log(newVersionStr, ConfigAuditLog.AuditAction.REJECT,
                            "quorum-timeout", previousVersionStr);

                    // Clean up ACKs for the failed version
                    quorumManager.cleanupAcks(newVersionStr);

                    return ConfigRolloutState.TIMED_OUT;
                }

            } catch (IllegalStateException | IllegalArgumentException e) {
                throw e;
            } catch (Exception e) {
                logger.error("Failed to propose configuration", e);
                throw new RuntimeException("Configuration proposal failed", e);
            }
        });
    }

    /**
     * Get the current active configuration.
     *
     * @return The current {@link ConfigurationContext}
     */
    public ConfigurationContext getCurrentConfig() {
        return currentConfig;
    }

    /**
     * Rollback to a previous configuration version.
     *
     * @param version The version number to roll back to
     * @throws Exception             If an error occurs during the rollback
     * @throws IllegalStateException If this node is not the leader
     */
    public void rollback(int version) throws Exception {
        if (!leaderElection.isLeader()) {
            throw new IllegalStateException("Only the leader can rollback configuration");
        }

        if (splitBrainDetector.isReadOnly()) {
            throw new IllegalStateException("Cannot rollback configuration in read-only mode");
        }

        String previousVersionStr = currentVersionString();
        String targetVersionStr = ConfigQuorumManager.formatVersion(version);

        logger.info("Rolling back configuration to version {}", targetVersionStr);

        // Verify the version exists and is readable
        ConfigurationContext rollbackConfig = versionStore.readVersion(version);

        // Update current pointer
        versionStore.setCurrent(version);

        // Update in-memory config
        this.currentConfig = rollbackConfig;

        // Save as LKG
        fallbackStore.saveLastKnownGood(rollbackConfig);

        // Log to audit trail
        auditLog.log(targetVersionStr, ConfigAuditLog.AuditAction.ROLLBACK,
                "operator-initiated-rollback", previousVersionStr);

        logger.info("Successfully rolled back configuration to version {}", targetVersionStr);
    }

    /**
     * List all available configuration versions.
     *
     * @return A list of version numbers in ascending order
     * @throws Exception If an error occurs reading from the backend
     */
    public List<Integer> listVersions() throws Exception {
        return versionStore.listVersions();
    }

    /**
     * Check if this node is the leader.
     *
     * @return {@code true} if this node holds the leadership
     */
    public boolean isLeader() {
        return leaderElection.isLeader();
    }

    /**
     * Get the quorum manager.
     *
     * @return The {@link ConfigQuorumManager} instance
     */
    public ConfigQuorumManager getQuorumManager() {
        return quorumManager;
    }

    /**
     * Get the audit log.
     *
     * @return The {@link ConfigAuditLog} instance
     */
    public ConfigAuditLog getAuditLog() {
        return auditLog;
    }

    /**
     * Set the quorum timeout duration.
     *
     * @param timeout The timeout duration for quorum acknowledgment
     */
    public void setQuorumTimeout(Duration timeout) {
        this.quorumTimeout = Objects.requireNonNull(timeout, "timeout");
    }

    /**
     * Set the cluster size used for quorum calculation.
     *
     * @param clusterSize The number of nodes in the cluster
     */
    public void setClusterSize(int clusterSize) {
        if (clusterSize < 1) {
            throw new IllegalArgumentException("clusterSize must be >= 1, got: " + clusterSize);
        }
        this.clusterSize = clusterSize;
    }

    private void onConfigChange(ConfigurationContext newConfig) {
        this.currentConfig = newConfig;
        try {
            fallbackStore.saveLastKnownGood(newConfig);
        } catch (Exception e) {
            logger.warn("Failed to save last-known-good after config change notification", e);
        }

        // Non-leader nodes: ACK the new version if possible
        if (!leaderElection.isLeader()) {
            try {
                int version = versionStore.currentVersion();
                if (version > 0) {
                    String versionStr = ConfigQuorumManager.formatVersion(version);
                    quorumManager.acknowledgeConfig(versionStr);
                    logger.info("Follower ACKed config version {}", versionStr);
                }
            } catch (Exception e) {
                logger.warn("Failed to ACK config version as follower", e);
            }
        }
    }

    private void onFallbackConfig(ConfigurationContext lkgConfig) {
        this.currentConfig = lkgConfig;
    }

    private String currentVersionString() {
        try {
            int v = versionStore.currentVersion();
            return v > 0 ? ConfigQuorumManager.formatVersion(v) : "none";
        } catch (Exception e) {
            return "unknown";
        }
    }

    @Override
    public void close() throws IOException {
        logger.info("Shutting down DistributedConfigurationManager");

        configWatcher.close();
        splitBrainDetector.close();
        quorumManager.close();
        auditLog.close();
        leaderElection.close();

        started = false;
        logger.info("DistributedConfigurationManager shut down");
    }
}
