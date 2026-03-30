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
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.Collection;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class NodeRegistryTest {

    private NodeRegistry registry;

    @BeforeEach
    void setUp() {
        registry = new NodeRegistry();
    }

    // --- Helper ---

    private static NodeIdentity identity(String nodeId) {
        return NodeIdentity.newBuilder()
                .setNodeId(nodeId)
                .setClusterId("cluster-1")
                .setEnvironment("test")
                .setAddress("10.0.0.1")
                .setBuildVersion("1.0.0")
                .build();
    }

    // --- register ---

    @Test
    void register_createsNodeInConnectedState() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-abc");

        assertNotNull(node);
        assertEquals("node-1", node.nodeId());
        assertEquals("cluster-1", node.clusterId());
        assertEquals("test", node.environment());
        assertEquals("10.0.0.1", node.address());
        assertEquals("1.0.0", node.buildVersion());
        assertEquals("token-abc", node.sessionToken());
        assertEquals(DataPlaneNodeState.CONNECTED, node.state());
    }

    @Test
    void register_nodeIsRetrievableViaGet() {
        DataPlaneNode registered = registry.register(identity("node-1"), "token-abc");

        Optional<DataPlaneNode> retrieved = registry.get("node-1");

        assertTrue(retrieved.isPresent());
        assertEquals(registered, retrieved.get());
    }

    @Test
    void register_duplicateNodeIdThrowsIllegalStateException() {
        registry.register(identity("node-1"), "token-1");

        assertThrows(IllegalStateException.class, () ->
                registry.register(identity("node-1"), "token-2"));
    }

    @Test
    void register_blankNodeIdThrowsIllegalArgumentException() {
        NodeIdentity blankId = NodeIdentity.newBuilder()
                .setNodeId("")
                .setClusterId("cluster-1")
                .setEnvironment("test")
                .setAddress("10.0.0.1")
                .setBuildVersion("1.0.0")
                .build();

        assertThrows(IllegalArgumentException.class, () ->
                registry.register(blankId, "token-abc"));
    }

    @Test
    void register_nullIdentityThrowsNullPointerException() {
        assertThrows(NullPointerException.class, () ->
                registry.register(null, "token-abc"));
    }

    @Test
    void register_nullSessionTokenThrowsNullPointerException() {
        assertThrows(NullPointerException.class, () ->
                registry.register(identity("node-1"), null));
    }

    // --- deregister ---

    @Test
    void deregister_removesNodeAndMarksDisconnected() {
        registry.register(identity("node-1"), "token-abc");

        DataPlaneNode removed = registry.deregister("node-1");

        assertNotNull(removed);
        assertEquals(DataPlaneNodeState.DISCONNECTED, removed.state());
        assertTrue(registry.get("node-1").isEmpty());
        assertEquals(0, registry.size());
    }

    @Test
    void deregister_unknownNodeReturnsNull() {
        DataPlaneNode result = registry.deregister("unknown-node");

        assertNull(result);
    }

    // --- get ---

    @Test
    void get_unknownNodeReturnsEmptyOptional() {
        Optional<DataPlaneNode> result = registry.get("nonexistent");

        assertTrue(result.isEmpty());
    }

    // --- validateSession ---

    @Test
    void validateSession_correctTokenReturnsTrue() {
        registry.register(identity("node-1"), "secret-token");

        assertTrue(registry.validateSession("node-1", "secret-token"));
    }

    @Test
    void validateSession_wrongTokenReturnsFalse() {
        registry.register(identity("node-1"), "secret-token");

        assertFalse(registry.validateSession("node-1", "wrong-token"));
    }

    @Test
    void validateSession_unknownNodeReturnsFalse() {
        assertFalse(registry.validateSession("unknown-node", "any-token"));
    }

    // --- allNodes ---

    @Test
    void allNodes_returnsUnmodifiableCollectionOfAllRegistered() {
        registry.register(identity("node-1"), "t1");
        registry.register(identity("node-2"), "t2");
        registry.register(identity("node-3"), "t3");

        Collection<DataPlaneNode> all = registry.allNodes();

        assertEquals(3, all.size());
        assertThrows(UnsupportedOperationException.class, () ->
                all.add(new DataPlaneNode(identity("hack"), "hack")));
    }

    @Test
    void allNodes_emptyRegistryReturnsEmptyCollection() {
        Collection<DataPlaneNode> all = registry.allNodes();

        assertTrue(all.isEmpty());
    }

    // --- healthyNodes ---

    @Test
    void healthyNodes_returnsOnlyHealthyNodes() {
        DataPlaneNode node1 = registry.register(identity("node-1"), "t1");
        registry.register(identity("node-2"), "t2");
        DataPlaneNode node3 = registry.register(identity("node-3"), "t3");

        // node-1: transition to HEALTHY via heartbeat
        node1.recordHeartbeat(1, 100, 0.5, 0.4);

        // node-2: stays CONNECTED (no heartbeat)

        // node-3: transition to HEALTHY, then mark UNHEALTHY
        node3.recordHeartbeat(1, 50, 0.3, 0.2);
        node3.markUnhealthy();

        List<DataPlaneNode> healthy = registry.healthyNodes();

        assertEquals(1, healthy.size());
        assertEquals("node-1", healthy.get(0).nodeId());
    }

    @Test
    void healthyNodes_noHealthyNodesReturnsEmptyList() {
        registry.register(identity("node-1"), "t1"); // CONNECTED, not HEALTHY

        List<DataPlaneNode> healthy = registry.healthyNodes();

        assertTrue(healthy.isEmpty());
    }

    // --- nodesByState ---

    @Test
    void nodesByState_filtersCorrectly() {
        DataPlaneNode node1 = registry.register(identity("node-1"), "t1");
        registry.register(identity("node-2"), "t2");
        DataPlaneNode node3 = registry.register(identity("node-3"), "t3");

        node1.recordHeartbeat(1, 0, 0, 0); // HEALTHY
        node3.markDraining();                // DRAINING

        assertEquals(1, registry.nodesByState(DataPlaneNodeState.CONNECTED).size());
        assertEquals(1, registry.nodesByState(DataPlaneNodeState.HEALTHY).size());
        assertEquals(1, registry.nodesByState(DataPlaneNodeState.DRAINING).size());
        assertEquals(0, registry.nodesByState(DataPlaneNodeState.UNHEALTHY).size());
        assertEquals(0, registry.nodesByState(DataPlaneNodeState.DISCONNECTED).size());
    }

    // --- countByState ---

    @Test
    void countByState_returnsCorrectCountsForAllStates() {
        registry.register(identity("node-1"), "t1");
        DataPlaneNode node2 = registry.register(identity("node-2"), "t2");
        DataPlaneNode node3 = registry.register(identity("node-3"), "t3");
        DataPlaneNode node4 = registry.register(identity("node-4"), "t4");

        // node-1: CONNECTED (default)
        // node-2: HEALTHY (via heartbeat)
        node2.recordHeartbeat(1, 0, 0, 0);
        // node-3: UNHEALTHY
        node3.markUnhealthy();
        // node-4: DRAINING
        node4.markDraining();

        Map<DataPlaneNodeState, Long> counts = registry.countByState();

        assertEquals(1L, counts.get(DataPlaneNodeState.CONNECTED));
        assertEquals(1L, counts.get(DataPlaneNodeState.HEALTHY));
        assertEquals(1L, counts.get(DataPlaneNodeState.UNHEALTHY));
        assertEquals(1L, counts.get(DataPlaneNodeState.DRAINING));
        assertEquals(0L, counts.get(DataPlaneNodeState.DISCONNECTED));
    }

    @Test
    void countByState_emptyRegistryReturnsAllZeros() {
        Map<DataPlaneNodeState, Long> counts = registry.countByState();

        for (DataPlaneNodeState state : DataPlaneNodeState.values()) {
            assertEquals(0L, counts.get(state));
        }
    }

    @Test
    void countByState_returnsUnmodifiableMap() {
        Map<DataPlaneNodeState, Long> counts = registry.countByState();

        assertThrows(UnsupportedOperationException.class, () ->
                counts.put(DataPlaneNodeState.HEALTHY, 999L));
    }

    // --- size ---

    @Test
    void size_reflectsRegistrationsAndDeregistrations() {
        assertEquals(0, registry.size());

        registry.register(identity("node-1"), "t1");
        assertEquals(1, registry.size());

        registry.register(identity("node-2"), "t2");
        assertEquals(2, registry.size());

        registry.deregister("node-1");
        assertEquals(1, registry.size());

        registry.deregister("node-2");
        assertEquals(0, registry.size());
    }

    // --- Multiple nodes, identity isolation ---

    @Test
    void multipleNodes_identitiesAreIsolated() {
        NodeIdentity id1 = NodeIdentity.newBuilder()
                .setNodeId("node-1")
                .setClusterId("cluster-A")
                .setEnvironment("prod")
                .setAddress("10.0.0.1")
                .setBuildVersion("2.0.0")
                .build();

        NodeIdentity id2 = NodeIdentity.newBuilder()
                .setNodeId("node-2")
                .setClusterId("cluster-B")
                .setEnvironment("staging")
                .setAddress("10.0.0.2")
                .setBuildVersion("3.0.0")
                .build();

        registry.register(id1, "token-1");
        registry.register(id2, "token-2");

        DataPlaneNode n1 = registry.get("node-1").orElseThrow();
        DataPlaneNode n2 = registry.get("node-2").orElseThrow();

        assertEquals("cluster-A", n1.clusterId());
        assertEquals("prod", n1.environment());

        assertEquals("cluster-B", n2.clusterId());
        assertEquals("staging", n2.environment());
    }
}
