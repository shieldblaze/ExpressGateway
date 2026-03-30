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
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Quorum-based configuration commit manager backed by a {@link ConfigStorageBackend}.
 *
 * <p>Ensures that a proposed configuration version is acknowledged by a majority of
 * registered cluster nodes before it is marked as COMMITTED. Each node that successfully
 * applies a config version creates an ephemeral key under the ACK path for that version.</p>
 *
 * <p>Key structure for ACKs:</p>
 * <pre>
 * /ExpressGateway/{env}/{clusterId}/config/acks/v001/node-{nodeId}
 * /ExpressGateway/{env}/{clusterId}/config/acks/v002/node-{nodeId}
 * </pre>
 *
 * <p>Ephemeral keys are used so that ACKs are automatically cleaned up when a node
 * disconnects, preventing stale acknowledgments from a crashed node from persisting.</p>
 *
 * <p>The leader calls {@link #awaitQuorum(String, int, Duration)} after writing a new
 * config version. If quorum is not reached within the timeout, the future completes
 * with {@code false}, signaling the caller to perform an automatic rollback.</p>
 */
public final class ConfigQuorumManager implements Closeable {

    private static final Logger logger = LogManager.getLogger(ConfigQuorumManager.class);

    private static final String ROOT_PATH = "ExpressGateway";
    private static final String CONFIG_COMPONENT = "config";
    private static final String ACKS_COMPONENT = "acks";
    private static final String VERSION_FORMAT = "v%03d";

    private static final Duration DEFAULT_TIMEOUT = Duration.ofSeconds(30);

    private final String clusterId;
    private final Environment environment;
    private final String nodeId;
    private final ConfigStorageBackend storageBackend;

    private final ScheduledExecutorService scheduler;
    private final ConcurrentMap<String, QuorumWaiter> activeWaiters;
    private final AtomicBoolean closed;
    private final Runnable connectionLossHandler;

    /**
     * Creates a new {@link ConfigQuorumManager}.
     *
     * @param clusterId      The cluster identifier
     * @param environment    The deployment environment
     * @param nodeId         The unique identifier for this node
     * @param storageBackend The storage backend
     */
    public ConfigQuorumManager(String clusterId, Environment environment, String nodeId,
                               ConfigStorageBackend storageBackend) {
        this.clusterId = Objects.requireNonNull(clusterId, "clusterId");
        this.environment = Objects.requireNonNull(environment, "environment");
        this.nodeId = Objects.requireNonNull(nodeId, "nodeId");
        this.storageBackend = Objects.requireNonNull(storageBackend, "storageBackend");
        this.scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "config-quorum-scheduler");
            t.setDaemon(true);
            return t;
        });
        this.activeWaiters = new ConcurrentHashMap<>();
        this.closed = new AtomicBoolean(false);

        // CRITICAL-2 fix: Register a connection loss listener that immediately fails
        // all active quorum waits on backend session loss. Without this, the quorum wait
        // could hang until timeout even though the watch listener is unreliable
        // after session expiration. Ephemeral ACK keys are deleted on session loss,
        // so any in-flight quorum is no longer achievable.
        this.connectionLossHandler = () -> {
            logger.warn("Backend connection lost, failing all active quorum waits");
            for (String version : activeWaiters.keySet()) {
                QuorumWaiter waiter = activeWaiters.get(version);
                if (waiter != null && !waiter.future.isDone()) {
                    waiter.future.complete(false);
                    cleanupWaiter(version);
                }
            }
        };
        storageBackend.addConnectionLossListener(connectionLossHandler);
    }

    /**
     * Wait for quorum acknowledgment of a configuration version.
     *
     * <p>The returned future completes with {@code true} if the required number of ACKs
     * is reached within the timeout, or {@code false} if the timeout expires first.
     * The future completes exceptionally if a backend error occurs.</p>
     *
     * @param version      The version identifier (e.g. "v005")
     * @param requiredAcks The number of acknowledgments required for quorum (majority)
     * @param timeout      The maximum duration to wait for quorum
     * @return A {@link CompletableFuture} that completes with the quorum result
     */
    public CompletableFuture<Boolean> awaitQuorum(String version, int requiredAcks, Duration timeout) {
        Objects.requireNonNull(version, "version");
        Objects.requireNonNull(timeout, "timeout");
        if (requiredAcks < 1) {
            throw new IllegalArgumentException("requiredAcks must be >= 1, got: " + requiredAcks);
        }

        if (closed.get()) {
            return CompletableFuture.completedFuture(false);
        }

        CompletableFuture<Boolean> future = new CompletableFuture<>();
        QuorumWaiter waiter = new QuorumWaiter(version, requiredAcks, future);
        activeWaiters.put(version, waiter);

        try {
            String acksPath = acksPathForVersion(version);

            // Ensure the ACKs parent key exists (persistent)
            storageBackend.put(acksPath, new byte[0]);

            // RACE FIX: Install the watch FIRST, then check the current ACK count.
            // This eliminates the window where an ACK arrives after the count check
            // but before the watch is active.
            Closeable watchHandle = storageBackend.watch(acksPath, data -> {
                if (future.isDone()) {
                    return;
                }
                try {
                    int count = getAckCountInternal(acksPath);
                    logger.debug("ACK received for version {}, count: {}/{}", version, count, requiredAcks);
                    if (count >= requiredAcks) {
                        logger.info("Quorum reached for version {} ({}/{})", version, count, requiredAcks);
                        future.complete(true);
                        cleanupWaiter(version);
                    }
                } catch (Exception e) {
                    logger.error("Error checking ACK count for version {}", version, e);
                }
            });
            waiter.watchHandle = watchHandle;

            // Check if quorum is already met
            int currentCount = getAckCountInternal(acksPath);
            if (currentCount >= requiredAcks) {
                logger.info("Quorum already reached for version {} ({}/{})", version, currentCount, requiredAcks);
                future.complete(true);
                cleanupWaiter(version);
                return future;
            }

            // Schedule timeout
            ScheduledFuture<?> timeoutFuture = scheduler.schedule(() -> {
                if (!future.isDone()) {
                    try {
                        int finalCount = getAckCountInternal(acksPath);
                        logger.warn("Quorum timeout for version {} ({}/{})", version, finalCount, requiredAcks);
                    } catch (Exception e) {
                        logger.warn("Could not read final ACK count for version {} on timeout", version);
                    }
                    future.complete(false);
                    cleanupWaiter(version);
                }
            }, timeout.toMillis(), TimeUnit.MILLISECONDS);

            waiter.timeoutFuture = timeoutFuture;

            // Handle external cancellation
            future.whenComplete((result, throwable) -> {
                timeoutFuture.cancel(false);
                cleanupWaiter(version);
            });

        } catch (Exception e) {
            logger.error("Failed to set up quorum waiting for version {}", version, e);
            future.completeExceptionally(e);
            activeWaiters.remove(version);
        }

        return future;
    }

    /**
     * Wait for quorum acknowledgment using the default timeout of 30 seconds.
     *
     * @param version      The version identifier (e.g. "v005")
     * @param requiredAcks The number of acknowledgments required for quorum
     * @return A {@link CompletableFuture} that completes with the quorum result
     */
    public CompletableFuture<Boolean> awaitQuorum(String version, int requiredAcks) {
        return awaitQuorum(version, requiredAcks, DEFAULT_TIMEOUT);
    }

    /**
     * Register this node's acknowledgment for a configuration version.
     *
     * <p>Creates an ephemeral key under the ACK path for the given version. The key
     * is named after this node's ID. If the node disconnects, the ephemeral key is
     * automatically removed by the backend.</p>
     *
     * @param version The version identifier (e.g. "v005")
     * @throws Exception If an error occurs creating the ACK key
     */
    public void acknowledgeConfig(String version) throws Exception {
        Objects.requireNonNull(version, "version");

        String acksPath = acksPathForVersion(version);
        String ackNodePath = acksPath + "/node-" + nodeId;

        // Create parent path if needed
        storageBackend.put(acksPath, new byte[0]);

        try {
            storageBackend.putEphemeral(ackNodePath, nodeId.getBytes(StandardCharsets.UTF_8));
            logger.info("Node {} acknowledged config version {}", nodeId, version);
        } catch (ConfigStorageBackend.KeyExistsException e) {
            // Idempotent: this node already acknowledged this version
            logger.debug("Node {} already acknowledged config version {}", nodeId, version);
        }
    }

    /**
     * Get the current ACK count for a configuration version.
     *
     * @param version The version identifier (e.g. "v005")
     * @return The number of ACKs for the given version, or 0 if the ACK path does not exist
     */
    public int getAckCount(String version) {
        Objects.requireNonNull(version, "version");
        try {
            String acksPath = acksPathForVersion(version);
            return getAckCountInternal(acksPath);
        } catch (Exception e) {
            logger.error("Failed to get ACK count for version {}", version, e);
            return 0;
        }
    }

    /**
     * Calculate the required quorum size (strict majority) for a given cluster size.
     *
     * @param totalNodes The total number of registered nodes in the cluster
     * @return The minimum number of ACKs required for quorum
     */
    public static int quorumSize(int totalNodes) {
        if (totalNodes <= 0) {
            throw new IllegalArgumentException("totalNodes must be > 0, got: " + totalNodes);
        }
        return (totalNodes / 2) + 1;
    }

    /**
     * Clean up ACK keys for a completed (committed or rolled-back) version.
     *
     * <p>This should be called after a version has been finalized to prevent
     * accumulation of stale ACK paths.</p>
     *
     * @param version The version identifier to clean up
     */
    public void cleanupAcks(String version) {
        Objects.requireNonNull(version, "version");
        try {
            String acksPath = acksPathForVersion(version);
            if (storageBackend.exists(acksPath)) {
                storageBackend.deleteTree(acksPath);
                logger.debug("Cleaned up ACK keys for version {}", version);
            }
        } catch (Exception e) {
            // Non-critical: stale ACK keys do not affect correctness
            logger.warn("Failed to clean up ACK keys for version {}", version, e);
        }
    }

    private int getAckCountInternal(String acksPath) throws Exception {
        if (!storageBackend.exists(acksPath)) {
            return 0;
        }
        List<String> children = storageBackend.listChildren(acksPath);
        return children.size();
    }

    private String acksPathForVersion(String version) {
        return ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT,
                ACKS_COMPONENT + "/" + version).path();
    }

    /**
     * Format a version number into the standard version string.
     *
     * @param versionNumber The numeric version
     * @return The formatted version string (e.g. "v005")
     */
    public static String formatVersion(int versionNumber) {
        return String.format(VERSION_FORMAT, versionNumber);
    }

    private void cleanupWaiter(String version) {
        QuorumWaiter waiter = activeWaiters.remove(version);
        if (waiter != null) {
            if (waiter.watchHandle != null) {
                try {
                    waiter.watchHandle.close();
                } catch (Exception e) {
                    logger.warn("Error closing watch handle for version {}", version, e);
                }
            }
            if (waiter.timeoutFuture != null) {
                waiter.timeoutFuture.cancel(false);
            }
        }
    }

    @Override
    public void close() throws IOException {
        if (closed.compareAndSet(false, true)) {
            // Remove connection loss listener
            storageBackend.removeConnectionLossListener(connectionLossHandler);

            // Complete all active waiters as failed
            for (String version : activeWaiters.keySet()) {
                QuorumWaiter waiter = activeWaiters.remove(version);
                if (waiter != null) {
                    waiter.future.complete(false);
                    if (waiter.watchHandle != null) {
                        try {
                            waiter.watchHandle.close();
                        } catch (Exception e) {
                            logger.warn("Error closing watch handle on shutdown", e);
                        }
                    }
                    if (waiter.timeoutFuture != null) {
                        waiter.timeoutFuture.cancel(false);
                    }
                }
            }
            scheduler.shutdown();
            try {
                if (!scheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                    scheduler.shutdownNow();
                }
            } catch (InterruptedException e) {
                scheduler.shutdownNow();
                Thread.currentThread().interrupt();
            }
            logger.info("Closed ConfigQuorumManager for node {}", nodeId);
        }
    }

    /**
     * Internal holder for an active quorum wait operation.
     */
    private static final class QuorumWaiter {
        final String version;
        final int requiredAcks;
        final CompletableFuture<Boolean> future;
        volatile Closeable watchHandle;
        volatile ScheduledFuture<?> timeoutFuture;

        QuorumWaiter(String version, int requiredAcks, CompletableFuture<Boolean> future) {
            this.version = version;
            this.requiredAcks = requiredAcks;
            this.future = future;
        }
    }
}
