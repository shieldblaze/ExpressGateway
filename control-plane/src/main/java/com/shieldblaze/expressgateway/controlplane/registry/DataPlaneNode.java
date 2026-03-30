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

import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import lombok.extern.log4j.Log4j2;

import java.time.Instant;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Represents a connected data-plane (load balancer) node as tracked by the control plane.
 *
 * <p>Thread-safety contract:
 * <ul>
 *   <li>Immutable identity fields are set once at construction and never change.</li>
 *   <li>Mutable operational fields ({@code state}, {@code lastHeartbeat}, {@code appliedConfigVersion},
 *       {@code activeConnections}, {@code cpuUtilization}, {@code memoryUtilization}) use
 *       {@code volatile} for lock-free visibility across threads.</li>
 *   <li>{@code missedHeartbeats} uses {@link AtomicInteger} for atomic increment-and-read.</li>
 * </ul>
 *
 * <p>This design avoids synchronization on the hot path (heartbeat recording and scanning)
 * while guaranteeing that all threads observe a consistent snapshot of each individual field.
 * Compound invariants (e.g. "state is HEALTHY and missedHeartbeats is 0") are not atomically
 * guarded because the heartbeat tracker is single-threaded and is the sole mutator of
 * {@code missedHeartbeats}; the gRPC handler is the sole caller of {@code recordHeartbeat}.</p>
 */
@Log4j2
public final class DataPlaneNode {

    // --- Immutable identity fields (set once at construction) ---

    private final String nodeId;
    private final String clusterId;
    private final String environment;
    private final String address;
    private final String buildVersion;
    private final Map<String, String> metadata;
    private final Instant connectedAt;
    private final String sessionToken;

    // --- Mutable operational fields (volatile for lock-free visibility) ---

    private volatile DataPlaneNodeState state;
    private volatile Instant lastHeartbeat;
    private volatile long appliedConfigVersion;
    private final AtomicInteger missedHeartbeats = new AtomicInteger(0);

    // --- Load metrics (volatile for lock-free reads from metrics/routing threads) ---

    private volatile long activeConnections;
    private volatile double cpuUtilization;
    private volatile double memoryUtilization;

    /**
     * Constructs a new {@code DataPlaneNode} from a proto-generated {@link NodeIdentity}
     * and a pre-generated session token.
     *
     * @param identity     the node identity from the registration request; must not be null
     * @param sessionToken the session token assigned to this node; must not be null or blank
     * @throws NullPointerException     if {@code identity} or {@code sessionToken} is null
     * @throws IllegalArgumentException if {@code sessionToken} is blank
     */
    public DataPlaneNode(NodeIdentity identity, String sessionToken) {
        Objects.requireNonNull(identity, "identity");
        Objects.requireNonNull(sessionToken, "sessionToken");
        if (sessionToken.isBlank()) {
            throw new IllegalArgumentException("sessionToken must not be blank");
        }

        this.nodeId = identity.getNodeId();
        this.clusterId = identity.getClusterId();
        this.environment = identity.getEnvironment();
        this.address = identity.getAddress();
        this.buildVersion = identity.getBuildVersion();
        this.metadata = Collections.unmodifiableMap(new LinkedHashMap<>(identity.getMetadataMap()));
        this.connectedAt = Instant.now();
        this.sessionToken = sessionToken;

        this.state = DataPlaneNodeState.CONNECTED;
        this.lastHeartbeat = this.connectedAt;
        this.appliedConfigVersion = 0L;
        this.activeConnections = 0L;
        this.cpuUtilization = 0.0;
        this.memoryUtilization = 0.0;
    }

    /**
     * Records a heartbeat from this node, updating all operational and load metrics.
     *
     * <p>Resets {@code missedHeartbeats} to 0 and transitions the node to {@link DataPlaneNodeState#HEALTHY}
     * if it was previously in {@link DataPlaneNodeState#CONNECTED} or {@link DataPlaneNodeState#UNHEALTHY}.</p>
     *
     * @param appliedVersion    the latest config version the node has applied
     * @param activeConnections the number of active connections on the node
     * @param cpu               CPU utilization (0.0 to 1.0)
     * @param mem               memory utilization (0.0 to 1.0)
     */
    public void recordHeartbeat(long appliedVersion, long activeConnections, double cpu, double mem) {
        this.missedHeartbeats.set(0);
        this.lastHeartbeat = Instant.now();
        this.appliedConfigVersion = appliedVersion;
        this.activeConnections = activeConnections;
        this.cpuUtilization = cpu;
        this.memoryUtilization = mem;

        DataPlaneNodeState current = this.state;
        if (current == DataPlaneNodeState.CONNECTED || current == DataPlaneNodeState.UNHEALTHY) {
            this.state = DataPlaneNodeState.HEALTHY;
            log.info("Node {} transitioned from {} to HEALTHY", nodeId, current);
        }
    }

    /**
     * Increments the missed heartbeat counter and returns the new count.
     *
     * <p>Called exclusively by the {@link HeartbeatTracker} scan thread.</p>
     *
     * @return the missed heartbeat count after incrementing
     */
    public int incrementMissedHeartbeats() {
        return missedHeartbeats.incrementAndGet();
    }

    /**
     * Transitions this node to {@link DataPlaneNodeState#UNHEALTHY}.
     * Called by the heartbeat tracker when missed heartbeats exceed the miss threshold.
     */
    public void markUnhealthy() {
        DataPlaneNodeState prev = this.state;
        this.state = DataPlaneNodeState.UNHEALTHY;
        log.info("Node {} transitioned from {} to UNHEALTHY", nodeId, prev);
    }

    /**
     * Transitions this node to {@link DataPlaneNodeState#HEALTHY}.
     * Used when a previously draining node is un-drained and should resume serving traffic.
     */
    public void markHealthy() {
        DataPlaneNodeState prev = this.state;
        this.state = DataPlaneNodeState.HEALTHY;
        log.info("Node {} transitioned from {} to HEALTHY", nodeId, prev);
    }

    /**
     * Transitions this node to {@link DataPlaneNodeState#DRAINING}.
     * A draining node should not receive new traffic but continues serving existing connections.
     */
    public void markDraining() {
        DataPlaneNodeState prev = this.state;
        this.state = DataPlaneNodeState.DRAINING;
        log.info("Node {} transitioned from {} to DRAINING", nodeId, prev);
    }

    /**
     * Transitions this node to {@link DataPlaneNodeState#DISCONNECTED}.
     * A disconnected node is no longer reachable and will be cleaned up.
     */
    public void markDisconnected() {
        DataPlaneNodeState prev = this.state;
        this.state = DataPlaneNodeState.DISCONNECTED;
        log.info("Node {} transitioned from {} to DISCONNECTED", nodeId, prev);
    }

    // --- Getters ---

    public String nodeId() {
        return nodeId;
    }

    public String clusterId() {
        return clusterId;
    }

    public String environment() {
        return environment;
    }

    public String address() {
        return address;
    }

    public String buildVersion() {
        return buildVersion;
    }

    public Map<String, String> metadata() {
        return Collections.unmodifiableMap(metadata);
    }

    public Instant connectedAt() {
        return connectedAt;
    }

    public String sessionToken() {
        return sessionToken;
    }

    public DataPlaneNodeState state() {
        return state;
    }

    public Instant lastHeartbeat() {
        return lastHeartbeat;
    }

    public long appliedConfigVersion() {
        return appliedConfigVersion;
    }

    public int missedHeartbeats() {
        return missedHeartbeats.get();
    }

    public long activeConnections() {
        return activeConnections;
    }

    public double cpuUtilization() {
        return cpuUtilization;
    }

    public double memoryUtilization() {
        return memoryUtilization;
    }

    @Override
    public String toString() {
        return "DataPlaneNode{" +
                "nodeId='" + nodeId + '\'' +
                ", clusterId='" + clusterId + '\'' +
                ", environment='" + environment + '\'' +
                ", address='" + address + '\'' +
                ", state=" + state +
                ", lastHeartbeat=" + lastHeartbeat +
                ", missedHeartbeats=" + missedHeartbeats.get() +
                ", appliedConfigVersion=" + appliedConfigVersion +
                ", activeConnections=" + activeConnections +
                ", cpu=" + String.format("%.2f", cpuUtilization) +
                ", mem=" + String.format("%.2f", memoryUtilization) +
                '}';
    }
}
