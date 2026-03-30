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
package com.shieldblaze.expressgateway.controlplane.cluster;

import lombok.extern.log4j.Log4j2;

import java.util.Comparator;
import java.util.Objects;
import java.util.Optional;

/**
 * Suggests the best control plane instance for a data-plane node based on
 * geographic proximity and instance health.
 *
 * <h3>Assignment strategy</h3>
 * <ol>
 *   <li><b>Region match</b> -- prefer a CP instance in the same region as the node.
 *       If the current instance is already in the same region, no reassignment is needed.</li>
 *   <li><b>Healthy peer preference</b> -- among same-region peers, prefer the one with
 *       the most recent heartbeat (proxy for liveness).</li>
 *   <li><b>Fallback</b> -- if no same-region peer exists, return empty. The node should
 *       stay connected to its current instance rather than being bounced to an arbitrary
 *       cross-region peer, which would increase latency.</li>
 * </ol>
 *
 * <p>This is <b>advisory</b>. Data-plane nodes can connect to ANY CP instance.
 * The suggestion is used in heartbeat responses to guide reconnection for
 * latency optimization, not for correctness.</p>
 *
 * <p>Thread safety: reads from the cluster's peer map, which is a
 * {@link java.util.concurrent.ConcurrentHashMap}.</p>
 */
@Log4j2
public final class RegionAwareNodeAssigner {

    private final ControlPlaneCluster cluster;

    /**
     * Creates a new region-aware node assigner.
     *
     * @param cluster the cluster manager providing peer information; must not be null
     */
    public RegionAwareNodeAssigner(ControlPlaneCluster cluster) {
        this.cluster = Objects.requireNonNull(cluster, "cluster");
    }

    /**
     * Suggests the best CP instance for a data-plane node in the given region.
     *
     * <p>Returns empty if the current instance is already optimal (same region)
     * or if no better instance is available. A non-empty result means the node
     * should consider reconnecting to the suggested instance for lower latency.</p>
     *
     * @param nodeRegion        the region of the data-plane node; must not be null or blank
     * @param currentInstanceId the ID of the CP instance the node is currently connected to;
     *                          must not be null or blank
     * @return a suggested CP instance to reconnect to, or empty if the current is optimal
     */
    public Optional<ControlPlaneInstance> suggestInstance(String nodeRegion, String currentInstanceId) {
        Objects.requireNonNull(nodeRegion, "nodeRegion");
        Objects.requireNonNull(currentInstanceId, "currentInstanceId");
        if (nodeRegion.isBlank()) {
            throw new IllegalArgumentException("nodeRegion must not be blank");
        }
        if (currentInstanceId.isBlank()) {
            throw new IllegalArgumentException("currentInstanceId must not be blank");
        }

        // If the current instance is in the same region, no reassignment needed.
        // This avoids unnecessary reconnection churn.
        ControlPlaneInstance current = findPeer(currentInstanceId);
        if (current != null && current.region().equals(nodeRegion)) {
            return Optional.empty();
        }

        // Find the healthiest same-region peer (most recent heartbeat).
        Optional<ControlPlaneInstance> bestPeer = cluster.peers().stream()
                .filter(peer -> peer.region().equals(nodeRegion))
                .max(Comparator.comparingLong(ControlPlaneInstance::lastHeartbeat));

        if (bestPeer.isPresent()) {
            ControlPlaneInstance suggested = bestPeer.get();
            // If the suggested peer is the same as the current, no reassignment needed
            if (suggested.instanceId().equals(currentInstanceId)) {
                return Optional.empty();
            }
            log.debug("Suggesting reassignment: nodeRegion={}, from={}, to={}",
                    nodeRegion, currentInstanceId, suggested.instanceId());
            return bestPeer;
        }

        // No same-region peer found. Keep the node where it is.
        return Optional.empty();
    }

    /**
     * Looks up a peer by instance ID.
     */
    private ControlPlaneInstance findPeer(String instanceId) {
        return cluster.peers().stream()
                .filter(p -> p.instanceId().equals(instanceId))
                .findFirst()
                .orElse(null);
    }
}
