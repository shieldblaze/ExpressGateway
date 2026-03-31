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

import java.time.Instant;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class DataPlaneNodeTest {

    private NodeIdentity defaultIdentity;

    @BeforeEach
    void setUp() {
        defaultIdentity = NodeIdentity.newBuilder()
                .setNodeId("node-1")
                .setClusterId("cluster-1")
                .setEnvironment("test")
                .setAddress("10.0.0.1")
                .setBuildVersion("1.0.0")
                .putMetadata("region", "us-east-1")
                .build();
    }

    private DataPlaneNode createNode() {
        return new DataPlaneNode(defaultIdentity, "session-token-123");
    }

    // --- Construction ---

    @Test
    void constructor_setsInitialStateToConnected() {
        DataPlaneNode node = createNode();

        assertEquals(DataPlaneNodeState.CONNECTED, node.state());
    }

    @Test
    void constructor_setsImmutableIdentityFields() {
        DataPlaneNode node = createNode();

        assertEquals("node-1", node.nodeId());
        assertEquals("cluster-1", node.clusterId());
        assertEquals("test", node.environment());
        assertEquals("10.0.0.1", node.address());
        assertEquals("1.0.0", node.buildVersion());
        assertEquals("session-token-123", node.sessionToken());
    }

    @Test
    void constructor_setsMetadataAsUnmodifiable() {
        DataPlaneNode node = createNode();

        assertEquals("us-east-1", node.metadata().get("region"));
        assertThrows(UnsupportedOperationException.class, () ->
                node.metadata().put("key", "value"));
    }

    @Test
    void constructor_setsInitialMetricsToZero() {
        DataPlaneNode node = createNode();

        assertEquals(0L, node.appliedConfigVersion());
        assertEquals(0L, node.activeConnections());
        assertEquals(0.0, node.cpuUtilization());
        assertEquals(0.0, node.memoryUtilization());
        assertEquals(0, node.missedHeartbeats());
    }

    @Test
    void constructor_setsConnectedAtTimestamp() {
        Instant before = Instant.now();
        DataPlaneNode node = createNode();
        Instant after = Instant.now();

        assertNotNull(node.connectedAt());
        assertTrue(!node.connectedAt().isBefore(before));
        assertTrue(!node.connectedAt().isAfter(after));
    }

    @Test
    void constructor_nullIdentityThrowsNullPointerException() {
        assertThrows(NullPointerException.class, () ->
                new DataPlaneNode(null, "token"));
    }

    @Test
    void constructor_nullSessionTokenThrowsNullPointerException() {
        assertThrows(NullPointerException.class, () ->
                new DataPlaneNode(defaultIdentity, null));
    }

    @Test
    void constructor_blankSessionTokenThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () ->
                new DataPlaneNode(defaultIdentity, ""));

        assertThrows(IllegalArgumentException.class, () ->
                new DataPlaneNode(defaultIdentity, "   "));
    }

    // --- recordHeartbeat ---

    @Test
    void recordHeartbeat_transitionsFromConnectedToHealthy() {
        DataPlaneNode node = createNode();
        assertEquals(DataPlaneNodeState.CONNECTED, node.state());

        node.recordHeartbeat(1, 100, 0.5, 0.4);

        assertEquals(DataPlaneNodeState.HEALTHY, node.state());
    }

    @Test
    void recordHeartbeat_transitionsFromUnhealthyToHealthy() {
        DataPlaneNode node = createNode();
        node.markUnhealthy();
        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());

        node.recordHeartbeat(1, 50, 0.3, 0.2);

        assertEquals(DataPlaneNodeState.HEALTHY, node.state());
    }

    @Test
    void recordHeartbeat_doesNotTransitionFromDraining() {
        DataPlaneNode node = createNode();
        node.markDraining();

        node.recordHeartbeat(1, 50, 0.3, 0.2);

        assertEquals(DataPlaneNodeState.DRAINING, node.state());
    }

    @Test
    void recordHeartbeat_doesNotTransitionFromDisconnected() {
        DataPlaneNode node = createNode();
        node.markDisconnected();

        node.recordHeartbeat(1, 50, 0.3, 0.2);

        assertEquals(DataPlaneNodeState.DISCONNECTED, node.state());
    }

    @Test
    void recordHeartbeat_staysHealthyWhenAlreadyHealthy() {
        DataPlaneNode node = createNode();
        node.recordHeartbeat(1, 10, 0.1, 0.1); // CONNECTED -> HEALTHY

        node.recordHeartbeat(2, 20, 0.2, 0.2); // HEALTHY -> HEALTHY

        assertEquals(DataPlaneNodeState.HEALTHY, node.state());
    }

    @Test
    void recordHeartbeat_updatesMetrics() {
        DataPlaneNode node = createNode();

        node.recordHeartbeat(42, 5000, 0.75, 0.60);

        assertEquals(42L, node.appliedConfigVersion());
        assertEquals(5000L, node.activeConnections());
        assertEquals(0.75, node.cpuUtilization(), 0.001);
        assertEquals(0.60, node.memoryUtilization(), 0.001);
    }

    @Test
    void recordHeartbeat_updatesLastHeartbeatTimestamp() {
        DataPlaneNode node = createNode();
        Instant initial = node.lastHeartbeat();

        // Small delay to ensure timestamp is distinct
        node.recordHeartbeat(1, 0, 0, 0);

        assertTrue(!node.lastHeartbeat().isBefore(initial));
    }

    @Test
    void recordHeartbeat_resetsMissedHeartbeatsToZero() {
        DataPlaneNode node = createNode();
        node.incrementMissedHeartbeats();
        node.incrementMissedHeartbeats();
        assertEquals(2, node.missedHeartbeats());

        node.recordHeartbeat(1, 0, 0, 0);

        assertEquals(0, node.missedHeartbeats());
    }

    // --- incrementMissedHeartbeats ---

    @Test
    void incrementMissedHeartbeats_incrementsAtomically() {
        DataPlaneNode node = createNode();

        assertEquals(1, node.incrementMissedHeartbeats());
        assertEquals(2, node.incrementMissedHeartbeats());
        assertEquals(3, node.incrementMissedHeartbeats());
        assertEquals(3, node.missedHeartbeats());
    }

    // --- State transitions ---

    @Test
    void markUnhealthy_changesStateToUnhealthy() {
        DataPlaneNode node = createNode();

        node.markUnhealthy();

        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());
    }

    @Test
    void markHealthy_changesStateToHealthy() {
        DataPlaneNode node = createNode();
        node.markDraining();

        node.markHealthy();

        assertEquals(DataPlaneNodeState.HEALTHY, node.state());
    }

    @Test
    void markDraining_changesStateToDraining() {
        DataPlaneNode node = createNode();
        node.recordHeartbeat(1, 0, 0, 0); // make HEALTHY first

        node.markDraining();

        assertEquals(DataPlaneNodeState.DRAINING, node.state());
    }

    @Test
    void markDisconnected_changesStateToDisconnected() {
        DataPlaneNode node = createNode();

        node.markDisconnected();

        assertEquals(DataPlaneNodeState.DISCONNECTED, node.state());
    }

    // --- toString ---

    @Test
    void toString_containsNodeIdAndState() {
        DataPlaneNode node = createNode();

        String str = node.toString();

        assertTrue(str.contains("node-1"));
        assertTrue(str.contains("CONNECTED"));
    }
}
