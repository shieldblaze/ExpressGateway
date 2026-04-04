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
package com.shieldblaze.expressgateway.controlplane.resilience;

import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneCluster;
import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneInstance;
import lombok.extern.log4j.Log4j2;

import java.time.Duration;
import java.time.Instant;
import java.util.Collection;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Detects network partitions and manages split-brain resolution using quorum-based decisions.
 *
 * <h3>Detection</h3>
 * <p>A partition is suspected when the number of reachable peers drops below
 * quorum (floor(total/2) + 1). The handler transitions through states:</p>
 * <ul>
 *   <li>{@link PartitionState#NORMAL} -- all peers reachable, quorum satisfied</li>
 *   <li>{@link PartitionState#SUSPECTED} -- peer count below quorum, grace period active</li>
 *   <li>{@link PartitionState#PARTITIONED} -- grace period expired, confirmed partition</li>
 *   <li>{@link PartitionState#DEGRADED} -- operating in degraded mode (read-only)</li>
 * </ul>
 *
 * <h3>Resolution</h3>
 * <p>The partition with quorum continues as the active partition. Instances in the
 * minority partition enter read-only mode and refuse config writes until the partition
 * heals and they rejoin the majority.</p>
 *
 * <p>Thread safety: state and suspectedSince are bundled into a single
 * {@link PartitionSnapshot} managed via {@link AtomicReference} with CAS,
 * ensuring they are always consistent.</p>
 */
@Log4j2
public final class PartitionHandler {

    /**
     * Partition state.
     */
    public enum PartitionState {
        /** All peers reachable, quorum satisfied. */
        NORMAL,
        /** Below quorum but within grace period. */
        SUSPECTED,
        /** Confirmed network partition after grace period. */
        PARTITIONED,
        /** Operating in degraded read-only mode. */
        DEGRADED
    }

    /**
     * Bundles partition state with the suspectedSince timestamp so they are
     * always updated atomically via CAS.
     *
     * @param state          the current partition state
     * @param suspectedSince when the partition was first suspected (null if NORMAL)
     */
    public record PartitionSnapshot(PartitionState state, Instant suspectedSince) {
        public PartitionSnapshot {
            Objects.requireNonNull(state, "state");
        }
    }

    /**
     * Listener for partition state changes.
     */
    @FunctionalInterface
    public interface PartitionListener {
        void onPartitionStateChange(PartitionState previous, PartitionState current, String reason);
    }

    private final ControlPlaneCluster cluster;
    private final int expectedClusterSize;
    private final Duration gracePeriod;
    private final AtomicReference<PartitionSnapshot> snapshot;
    private final java.util.concurrent.CopyOnWriteArrayList<PartitionListener> listeners =
            new java.util.concurrent.CopyOnWriteArrayList<>();

    /**
     * Creates a new partition handler.
     *
     * @param cluster             the cluster to monitor
     * @param expectedClusterSize the expected total number of CP instances
     * @param gracePeriod         how long to wait after quorum loss before confirming partition
     */
    public PartitionHandler(ControlPlaneCluster cluster, int expectedClusterSize, Duration gracePeriod) {
        this.cluster = Objects.requireNonNull(cluster, "cluster");
        if (expectedClusterSize < 1) {
            throw new IllegalArgumentException("expectedClusterSize must be >= 1");
        }
        this.expectedClusterSize = expectedClusterSize;
        this.gracePeriod = Objects.requireNonNull(gracePeriod, "gracePeriod");
        this.snapshot = new AtomicReference<>(new PartitionSnapshot(PartitionState.NORMAL, null));
    }

    /**
     * Adds a listener for partition state changes.
     */
    public void addListener(PartitionListener listener) {
        Objects.requireNonNull(listener, "listener");
        listeners.add(listener);
    }

    /**
     * Returns the current partition state.
     */
    public PartitionState currentState() {
        return snapshot.get().state();
    }

    /**
     * Returns the current partition snapshot (state + suspectedSince).
     */
    public PartitionSnapshot currentSnapshot() {
        return snapshot.get();
    }

    /**
     * Returns whether the current instance has quorum.
     */
    public boolean hasQuorum() {
        return reachablePeerCount() >= quorumSize();
    }

    /**
     * Returns the quorum size required for this cluster.
     * Quorum = floor(expectedClusterSize / 2) + 1
     */
    public int quorumSize() {
        return (expectedClusterSize / 2) + 1;
    }

    /**
     * Returns the number of reachable peers (including self).
     */
    public int reachablePeerCount() {
        Collection<ControlPlaneInstance> peers = cluster.peers();
        return peers.size();
    }

    /**
     * Returns whether config writes should be allowed in the current state.
     * Writes are only allowed in NORMAL state or when this instance has quorum.
     */
    public boolean isWriteAllowed() {
        PartitionState current = snapshot.get().state();
        return current == PartitionState.NORMAL ||
               (current == PartitionState.SUSPECTED && hasQuorum());
    }

    /**
     * Evaluates the current cluster state and transitions the partition handler
     * accordingly. Should be called periodically (e.g., on each heartbeat cycle).
     *
     * <p>All state transitions use CAS to ensure atomicity of state + suspectedSince.</p>
     *
     * @return the new partition state after evaluation
     */
    public PartitionState evaluate() {
        int peerCount = reachablePeerCount();
        int quorum = quorumSize();

        // CAS loop: read current snapshot, decide new state, attempt transition
        while (true) {
            PartitionSnapshot current = snapshot.get();
            PartitionState currentState = current.state();

            if (peerCount >= quorum) {
                // Quorum satisfied
                if (currentState != PartitionState.NORMAL) {
                    PartitionSnapshot next = new PartitionSnapshot(PartitionState.NORMAL, null);
                    if (snapshot.compareAndSet(current, next)) {
                        fireStateChange(currentState, PartitionState.NORMAL,
                                "Quorum restored: " + peerCount + "/" + expectedClusterSize + " peers reachable");
                        return PartitionState.NORMAL;
                    }
                    continue; // CAS failed, retry
                }
                return PartitionState.NORMAL;
            }

            // Below quorum
            if (currentState == PartitionState.NORMAL) {
                // Enter suspected state with fresh suspectedSince
                PartitionSnapshot next = new PartitionSnapshot(PartitionState.SUSPECTED, Instant.now());
                if (snapshot.compareAndSet(current, next)) {
                    fireStateChange(currentState, PartitionState.SUSPECTED,
                            "Quorum lost: " + peerCount + "/" + expectedClusterSize + " peers reachable, " +
                            "grace period " + gracePeriod);
                    return PartitionState.SUSPECTED;
                }
                continue; // CAS failed, retry
            }

            if (currentState == PartitionState.SUSPECTED) {
                Instant suspectedSince = current.suspectedSince();
                // Check if grace period has expired
                if (suspectedSince != null &&
                    Duration.between(suspectedSince, Instant.now()).compareTo(gracePeriod) > 0) {

                    PartitionState targetState;
                    String reason;
                    if (cluster.isLeader()) {
                        targetState = PartitionState.DEGRADED;
                        reason = "Grace period expired, leader operating in degraded mode";
                    } else {
                        targetState = PartitionState.PARTITIONED;
                        reason = "Grace period expired, confirmed partition (follower)";
                    }

                    PartitionSnapshot next = new PartitionSnapshot(targetState, suspectedSince);
                    if (snapshot.compareAndSet(current, next)) {
                        fireStateChange(currentState, targetState, reason);
                        return targetState;
                    }
                    continue; // CAS failed, retry
                }
                // Still within grace period
                return PartitionState.SUSPECTED;
            }

            // Already partitioned or degraded -- remain until quorum is restored
            return currentState;
        }
    }

    private void fireStateChange(PartitionState from, PartitionState to, String reason) {
        log.warn("Partition state change: {} -> {} (reason: {})", from, to, reason);
        for (PartitionListener listener : listeners) {
            try {
                listener.onPartitionStateChange(from, to, reason);
            } catch (Exception e) {
                log.error("Partition listener threw exception", e);
            }
        }
    }
}
