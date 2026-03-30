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

import java.nio.charset.StandardCharsets;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;
import java.security.MessageDigest;
import java.util.Collection;
import java.util.Collections;
import java.util.EnumMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Thread-safe registry of all data-plane nodes connected to this control plane instance.
 *
 * <p>Backed by a {@link ConcurrentHashMap} keyed on {@code nodeId}. All operations are
 * lock-free for reads and use CAS-style insertion for registration to prevent duplicate
 * node IDs without external synchronization.</p>
 *
 * <p>Session tokens are generated at registration time using {@link UUID#randomUUID()}.
 * This is sufficient for development and testing; production deployments should replace
 * this with HMAC-SHA256 or a proper token service.</p>
 */
@Log4j2
public final class NodeRegistry {

    private final ConcurrentHashMap<String, DataPlaneNode> nodes = new ConcurrentHashMap<>();

    /**
     * Registers a new data-plane node.
     *
     * <p>Generates a session token via {@link UUID#randomUUID()} and creates a new
     * {@link DataPlaneNode} in {@link DataPlaneNodeState#CONNECTED} state.</p>
     *
     * @param identity     the node identity from the registration request; must not be null
     * @param sessionToken the session token assigned to this node; must not be null or blank
     * @return the newly created {@link DataPlaneNode}
     * @throws NullPointerException     if {@code identity} or {@code sessionToken} is null
     * @throws IllegalStateException    if a node with the same {@code nodeId} is already registered
     * @throws IllegalArgumentException if the node ID is blank
     */
    public DataPlaneNode register(NodeIdentity identity, String sessionToken) {
        Objects.requireNonNull(identity, "identity");
        Objects.requireNonNull(sessionToken, "sessionToken");

        String nodeId = identity.getNodeId();
        if (nodeId == null || nodeId.isBlank()) {
            throw new IllegalArgumentException("NodeIdentity.nodeId must not be blank");
        }

        DataPlaneNode node = new DataPlaneNode(identity, sessionToken);
        DataPlaneNode existing = nodes.putIfAbsent(nodeId, node);
        if (existing != null) {
            throw new IllegalStateException("Node already registered: " + nodeId);
        }

        log.info("Registered node: {} (cluster={}, env={}, addr={})",
                nodeId, identity.getClusterId(), identity.getEnvironment(), identity.getAddress());
        return node;
    }

    /**
     * Deregisters a node by its ID, marking it as {@link DataPlaneNodeState#DISCONNECTED}.
     *
     * @param nodeId the node ID to remove; must not be null
     * @return the removed {@link DataPlaneNode}, or {@code null} if no node was found
     */
    public DataPlaneNode deregister(String nodeId) {
        Objects.requireNonNull(nodeId, "nodeId");

        DataPlaneNode removed = nodes.remove(nodeId);
        if (removed != null) {
            removed.markDisconnected();
            log.info("Deregistered node: {}", sanitize(nodeId));
        } else {
            log.warn("Attempted to deregister unknown node: {}", sanitize(nodeId));
        }
        return removed;
    }

    /**
     * Retrieves a node by its ID.
     *
     * @param nodeId the node ID to look up; must not be null
     * @return an {@link Optional} containing the node, or empty if not found
     */
    public Optional<DataPlaneNode> get(String nodeId) {
        Objects.requireNonNull(nodeId, "nodeId");
        return Optional.ofNullable(nodes.get(nodeId));
    }

    /**
     * Validates that the provided session token matches the one stored for the given node.
     *
     * <p>Uses {@link MessageDigest#isEqual(byte[], byte[])} for constant-time comparison
     * to prevent timing side-channel attacks on session token validation.</p>
     *
     * @param nodeId       the node ID; must not be null
     * @param sessionToken the session token to validate; must not be null
     * @return {@code true} if the node exists and the token matches, {@code false} otherwise
     */
    public boolean validateSession(String nodeId, String sessionToken) {
        Objects.requireNonNull(nodeId, "nodeId");
        Objects.requireNonNull(sessionToken, "sessionToken");

        DataPlaneNode node = nodes.get(nodeId);
        if (node == null) {
            return false;
        }
        return MessageDigest.isEqual(
                node.sessionToken().getBytes(StandardCharsets.UTF_8),
                sessionToken.getBytes(StandardCharsets.UTF_8));
    }

    /**
     * Returns an unmodifiable view of all registered nodes.
     *
     * <p>The returned collection is a snapshot of the values at the time of the call.
     * Concurrent modifications to the registry will not be reflected.</p>
     *
     * @return unmodifiable collection of all nodes
     */
    public Collection<DataPlaneNode> allNodes() {
        return Collections.unmodifiableCollection(nodes.values());
    }

    /**
     * Returns all nodes currently in the specified state.
     *
     * @param state the state to filter by; must not be null
     * @return list of nodes in the given state (may be empty, never null)
     */
    public List<DataPlaneNode> nodesByState(DataPlaneNodeState state) {
        Objects.requireNonNull(state, "state");
        return nodes.values().stream()
                .filter(node -> node.state() == state)
                .toList();
    }

    /**
     * Returns all nodes in {@link DataPlaneNodeState#HEALTHY} state.
     *
     * @return list of healthy nodes (may be empty, never null)
     */
    public List<DataPlaneNode> healthyNodes() {
        return nodesByState(DataPlaneNodeState.HEALTHY);
    }

    /**
     * Returns a count of nodes grouped by state.
     *
     * @return map from each {@link DataPlaneNodeState} to the number of nodes in that state
     */
    public Map<DataPlaneNodeState, Long> countByState() {
        Map<DataPlaneNodeState, Long> counts = new EnumMap<>(DataPlaneNodeState.class);
        for (DataPlaneNodeState s : DataPlaneNodeState.values()) {
            counts.put(s, 0L);
        }
        for (DataPlaneNode node : nodes.values()) {
            counts.merge(node.state(), 1L, Long::sum);
        }
        return Collections.unmodifiableMap(counts);
    }

    /**
     * Returns the total number of registered nodes (all states).
     *
     * @return node count
     */
    public int size() {
        return nodes.size();
    }
}
