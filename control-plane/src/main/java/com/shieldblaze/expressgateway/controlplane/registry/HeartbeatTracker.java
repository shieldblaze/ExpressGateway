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
package com.shieldblaze.expressgateway.controlplane.registry;

import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.time.Instant;
import java.util.Iterator;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Periodically scans all registered data-plane nodes and increments their missed heartbeat
 * counters. Nodes that exceed configurable thresholds are transitioned to
 * {@link DataPlaneNodeState#UNHEALTHY} or {@link DataPlaneNodeState#DISCONNECTED}.
 *
 * <p>Design notes:
 * <ul>
 *   <li>Uses a single-threaded {@link ScheduledExecutorService} with a daemon thread.
 *       The scan is O(N) over registered nodes but each iteration is trivial (volatile read +
 *       atomic increment), so this scales comfortably to 10K+ nodes.</li>
 *   <li>No locks are acquired during the scan. State transitions use volatile writes on
 *       {@link DataPlaneNode}, and the missed heartbeat counter uses {@link java.util.concurrent.atomic.AtomicInteger}.</li>
 *   <li>Nodes in {@link DataPlaneNodeState#DRAINING} or {@link DataPlaneNodeState#DISCONNECTED}
 *       state are skipped -- they are managed by explicit lifecycle commands, not heartbeats.</li>
 * </ul>
 */
@Log4j2
public final class HeartbeatTracker implements Closeable {

    /** Default: mark UNHEALTHY after 3 consecutive missed heartbeats. */
    public static final int DEFAULT_MISS_THRESHOLD = 3;

    /** Default: mark DISCONNECTED after 6 consecutive missed heartbeats. */
    public static final int DEFAULT_DISCONNECT_THRESHOLD = 6;

    /** Default: scan every 5 seconds. */
    public static final long DEFAULT_SCAN_INTERVAL_MS = 5000L;

    /** Default: remove DISCONNECTED nodes after 5 minutes (300,000 ms). */
    public static final long DEFAULT_DISCONNECTED_CLEANUP_DELAY_MS = 300_000L;

    private final int missThreshold;
    private final int disconnectThreshold;
    private final long scanIntervalMs;
    private final long disconnectedNodeCleanupDelayMs;
    private final NodeRegistry registry;
    private final ScheduledExecutorService scheduler;
    private final AtomicBoolean running = new AtomicBoolean(false);

    /**
     * Tracks nodes pending removal after being marked DISCONNECTED.
     * Key: nodeId, Value: the {@link Instant} when the node was first observed as DISCONNECTED.
     * Nodes in this map will be deregistered from the {@link NodeRegistry} once the
     * cleanup delay has elapsed.
     */
    private final ConcurrentHashMap<String, Instant> pendingRemoval = new ConcurrentHashMap<>();

    /**
     * Creates a new heartbeat tracker with the specified thresholds and cleanup delay.
     *
     * @param registry                      the node registry to scan; must not be null
     * @param missThreshold                 consecutive missed heartbeats before marking UNHEALTHY; must be >= 1
     * @param disconnectThreshold           consecutive missed heartbeats before marking DISCONNECTED; must be > missThreshold
     * @param scanIntervalMs                interval between scans in milliseconds; must be >= 100
     * @param disconnectedNodeCleanupDelayMs delay before removing DISCONNECTED nodes from the registry in ms; must be >= 0
     * @throws IllegalArgumentException if thresholds or interval are invalid
     */
    public HeartbeatTracker(NodeRegistry registry, int missThreshold, int disconnectThreshold,
                            long scanIntervalMs, long disconnectedNodeCleanupDelayMs) {
        this.registry = Objects.requireNonNull(registry, "registry");

        if (missThreshold < 1) {
            throw new IllegalArgumentException("missThreshold must be >= 1, got: " + missThreshold);
        }
        if (disconnectThreshold <= missThreshold) {
            throw new IllegalArgumentException(
                    "disconnectThreshold (" + disconnectThreshold + ") must be > missThreshold (" + missThreshold + ")");
        }
        if (scanIntervalMs < 100) {
            throw new IllegalArgumentException("scanIntervalMs must be >= 100, got: " + scanIntervalMs);
        }
        if (disconnectedNodeCleanupDelayMs < 0) {
            throw new IllegalArgumentException("disconnectedNodeCleanupDelayMs must be >= 0, got: " + disconnectedNodeCleanupDelayMs);
        }

        this.missThreshold = missThreshold;
        this.disconnectThreshold = disconnectThreshold;
        this.scanIntervalMs = scanIntervalMs;
        this.disconnectedNodeCleanupDelayMs = disconnectedNodeCleanupDelayMs;

        this.scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "cp-heartbeat-tracker");
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Creates a new heartbeat tracker with the specified thresholds and default cleanup delay.
     *
     * @param registry            the node registry to scan; must not be null
     * @param missThreshold       consecutive missed heartbeats before marking UNHEALTHY; must be >= 1
     * @param disconnectThreshold consecutive missed heartbeats before marking DISCONNECTED; must be > missThreshold
     * @param scanIntervalMs      interval between scans in milliseconds; must be >= 100
     * @throws IllegalArgumentException if thresholds or interval are invalid
     */
    public HeartbeatTracker(NodeRegistry registry, int missThreshold, int disconnectThreshold, long scanIntervalMs) {
        this(registry, missThreshold, disconnectThreshold, scanIntervalMs, DEFAULT_DISCONNECTED_CLEANUP_DELAY_MS);
    }

    /**
     * Convenience constructor using default thresholds (miss=3, disconnect=6, interval=5000ms,
     * cleanup=300000ms).
     *
     * @param registry the node registry to scan; must not be null
     */
    public HeartbeatTracker(NodeRegistry registry) {
        this(registry, DEFAULT_MISS_THRESHOLD, DEFAULT_DISCONNECT_THRESHOLD,
                DEFAULT_SCAN_INTERVAL_MS, DEFAULT_DISCONNECTED_CLEANUP_DELAY_MS);
    }

    /**
     * Starts the periodic heartbeat scan.
     *
     * @throws IllegalStateException if already started
     */
    public void start() {
        if (!running.compareAndSet(false, true)) {
            throw new IllegalStateException("HeartbeatTracker is already running");
        }
        scheduler.scheduleAtFixedRate(this::scan, scanIntervalMs, scanIntervalMs, TimeUnit.MILLISECONDS);
        log.info("HeartbeatTracker started: scanInterval={}ms, missThreshold={}, disconnectThreshold={}",
                scanIntervalMs, missThreshold, disconnectThreshold);
    }

    /**
     * Executes a single scan pass over all registered nodes.
     *
     * <p>For each node that is not in DRAINING or DISCONNECTED state, increments the
     * missed heartbeat counter and checks against thresholds. This method is called
     * by the scheduled executor but can also be invoked directly for testing.</p>
     *
     * <p>After the heartbeat check pass, a second pass cleans up DISCONNECTED nodes
     * whose cleanup delay has elapsed, deregistering them from the {@link NodeRegistry}.</p>
     */
    void scan() {
        try {
            for (DataPlaneNode node : registry.allNodes()) {
                DataPlaneNodeState currentState = node.state();

                // Skip nodes that are already draining or disconnected -- those are managed
                // by explicit lifecycle commands, not the heartbeat scanner.
                if (currentState == DataPlaneNodeState.DRAINING || currentState == DataPlaneNodeState.DISCONNECTED) {
                    // If DISCONNECTED, schedule for deferred removal (idempotent putIfAbsent)
                    if (currentState == DataPlaneNodeState.DISCONNECTED) {
                        pendingRemoval.putIfAbsent(node.nodeId(), Instant.now());
                    }
                    continue;
                }

                int missed = node.incrementMissedHeartbeats();

                if (missed >= disconnectThreshold) {
                    node.markDisconnected();
                    log.warn("Node {} marked DISCONNECTED after {} missed heartbeats (threshold={})",
                            node.nodeId(), missed, disconnectThreshold);
                    // Schedule deferred removal
                    pendingRemoval.putIfAbsent(node.nodeId(), Instant.now());
                } else if (missed >= missThreshold && currentState != DataPlaneNodeState.UNHEALTHY) {
                    // Transition to UNHEALTHY. We only log on the first transition, not on
                    // every subsequent scan while the node remains unhealthy.
                    // Direct volatile write -- the node is already not DRAINING/DISCONNECTED.
                    node.markUnhealthy();
                    log.info("Node {} marked UNHEALTHY after {} missed heartbeats (threshold={})",
                            node.nodeId(), missed, missThreshold);
                }
            }

            // Cleanup pass: remove DISCONNECTED nodes past the cleanup threshold.
            cleanupDisconnectedNodes();

        } catch (Exception e) {
            // Never let an exception kill the scheduled task. Log and continue.
            log.error("Error during heartbeat scan", e);
        }
    }

    /**
     * Removes DISCONNECTED nodes from the registry after the configured cleanup delay.
     *
     * <p>If a node that was scheduled for removal has been re-registered or is no longer
     * DISCONNECTED (e.g., it reconnected and transitioned back to HEALTHY), it is
     * removed from the pending removal map without deregistration.</p>
     */
    private void cleanupDisconnectedNodes() {
        if (pendingRemoval.isEmpty()) {
            return;
        }

        Instant cutoff = Instant.now().minusMillis(disconnectedNodeCleanupDelayMs);
        Iterator<Map.Entry<String, Instant>> it = pendingRemoval.entrySet().iterator();

        while (it.hasNext()) {
            Map.Entry<String, Instant> entry = it.next();
            String nodeId = entry.getKey();
            Instant disconnectedAt = entry.getValue();

            // Check if the node is still registered and still DISCONNECTED
            var nodeOpt = registry.get(nodeId);
            if (nodeOpt.isEmpty()) {
                // Already removed externally (e.g., explicit deregister RPC)
                it.remove();
                continue;
            }

            DataPlaneNode node = nodeOpt.get();
            if (node.state() != DataPlaneNodeState.DISCONNECTED) {
                // Node reconnected or state changed -- cancel pending removal
                it.remove();
                log.info("Node {} recovered from DISCONNECTED (now {}), cancelling deferred removal",
                        nodeId, node.state());
                continue;
            }

            if (disconnectedAt.isBefore(cutoff)) {
                registry.deregister(nodeId);
                it.remove();
                log.info("Node {} deregistered after {}ms in DISCONNECTED state (cleanup delay={}ms)",
                        nodeId, java.time.Duration.between(disconnectedAt, Instant.now()).toMillis(),
                        disconnectedNodeCleanupDelayMs);
            }
        }
    }

    /**
     * Returns whether this tracker is currently running.
     *
     * @return {@code true} if started and not yet closed
     */
    public boolean isRunning() {
        return running.get();
    }

    @Override
    public void close() {
        running.set(false);
        scheduler.shutdown();
        try {
            if (!scheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                scheduler.shutdownNow();
                log.warn("HeartbeatTracker scheduler did not terminate within 5 seconds, forced shutdown");
            }
        } catch (InterruptedException e) {
            scheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }
        log.info("HeartbeatTracker stopped");
    }
}
