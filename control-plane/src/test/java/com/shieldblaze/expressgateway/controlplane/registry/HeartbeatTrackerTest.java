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
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HeartbeatTrackerTest {

    private NodeRegistry registry;
    private HeartbeatTracker tracker;

    @BeforeEach
    void setUp() {
        registry = new NodeRegistry();
    }

    @AfterEach
    void tearDown() {
        if (tracker != null) {
            tracker.close();
        }
    }

    // --- Helpers ---

    private static NodeIdentity identity(String nodeId) {
        return NodeIdentity.newBuilder()
                .setNodeId(nodeId)
                .setClusterId("cluster-1")
                .setEnvironment("test")
                .setAddress("10.0.0.1")
                .setBuildVersion("1.0.0")
                .build();
    }

    private HeartbeatTracker createTracker(int missThreshold, int disconnectThreshold) {
        tracker = new HeartbeatTracker(registry, missThreshold, disconnectThreshold, 5000);
        return tracker;
    }

    // --- Constructor validation ---

    @Test
    void constructor_missThresholdZeroThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                new HeartbeatTracker(registry, 0, 3, 5000));
    }

    @Test
    void constructor_missThresholdNegativeThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                new HeartbeatTracker(registry, -1, 3, 5000));
    }

    @Test
    void constructor_disconnectThresholdEqualToMissThresholdThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                new HeartbeatTracker(registry, 3, 3, 5000));
    }

    @Test
    void constructor_disconnectThresholdLessThanMissThresholdThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                new HeartbeatTracker(registry, 3, 2, 5000));
    }

    @Test
    void constructor_scanIntervalMsBelowMinimumThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                new HeartbeatTracker(registry, 1, 2, 99));
    }

    @Test
    void constructor_scanIntervalMsExactMinimumSucceeds() {
        tracker = new HeartbeatTracker(registry, 1, 2, 100);
        assertFalse(tracker.isRunning());
    }

    @Test
    void constructor_nullRegistryThrows() {
        assertThrows(NullPointerException.class, () ->
                new HeartbeatTracker(null, 3, 6, 5000));
    }

    @Test
    void constructor_defaultThresholdsSucceeds() {
        tracker = new HeartbeatTracker(registry);
        assertFalse(tracker.isRunning());
    }

    // --- scan() - missed heartbeat increments ---

    @Test
    void scan_incrementsMissedHeartbeatsOnActiveNodes() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        createTracker(3, 6);

        tracker.scan();

        assertEquals(1, node.missedHeartbeats());

        tracker.scan();

        assertEquals(2, node.missedHeartbeats());
    }

    @Test
    void scan_skipsDrainingNodes() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        node.markDraining();
        createTracker(3, 6);

        tracker.scan();
        tracker.scan();
        tracker.scan();

        // Missed heartbeats should not be incremented for DRAINING nodes
        assertEquals(0, node.missedHeartbeats());
        assertEquals(DataPlaneNodeState.DRAINING, node.state());
    }

    @Test
    void scan_skipsDisconnectedNodes() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        node.markDisconnected();
        createTracker(3, 6);

        tracker.scan();
        tracker.scan();
        tracker.scan();

        // Missed heartbeats should not be incremented for DISCONNECTED nodes
        assertEquals(0, node.missedHeartbeats());
        assertEquals(DataPlaneNodeState.DISCONNECTED, node.state());
    }

    // --- scan() - state transitions ---

    @Test
    void scan_marksUnhealthyAfterMissThreshold() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        node.recordHeartbeat(1, 0, 0, 0); // CONNECTED -> HEALTHY
        createTracker(3, 6);

        // 1st and 2nd miss: still HEALTHY
        tracker.scan();
        assertEquals(DataPlaneNodeState.HEALTHY, node.state());
        tracker.scan();
        assertEquals(DataPlaneNodeState.HEALTHY, node.state());

        // 3rd miss: reaches threshold -> UNHEALTHY
        tracker.scan();
        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());
        assertEquals(3, node.missedHeartbeats());
    }

    @Test
    void scan_marksDisconnectedAfterDisconnectThreshold() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        node.recordHeartbeat(1, 0, 0, 0); // CONNECTED -> HEALTHY
        createTracker(2, 4);

        // 1st miss: HEALTHY
        tracker.scan();
        assertEquals(DataPlaneNodeState.HEALTHY, node.state());

        // 2nd miss: missThreshold reached -> UNHEALTHY
        tracker.scan();
        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());

        // 3rd miss: still UNHEALTHY
        tracker.scan();
        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());

        // 4th miss: disconnectThreshold reached -> DISCONNECTED
        tracker.scan();
        assertEquals(DataPlaneNodeState.DISCONNECTED, node.state());
        assertEquals(4, node.missedHeartbeats());
    }

    @Test
    void scan_connectedNodeBecomesUnhealthyThenDisconnected() {
        // A CONNECTED node (never had a heartbeat) also gets scanned
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        createTracker(1, 3);

        // 1st miss: reaches missThreshold -> UNHEALTHY
        tracker.scan();
        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());

        // 2nd miss: still UNHEALTHY
        tracker.scan();
        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());

        // 3rd miss: reaches disconnectThreshold -> DISCONNECTED
        tracker.scan();
        assertEquals(DataPlaneNodeState.DISCONNECTED, node.state());
    }

    @Test
    void scan_onceDisconnectedNodeIsSkipped() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        createTracker(1, 2);

        // 1st miss: UNHEALTHY
        tracker.scan();
        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());

        // 2nd miss: DISCONNECTED
        tracker.scan();
        assertEquals(DataPlaneNodeState.DISCONNECTED, node.state());
        int missedAtDisconnect = node.missedHeartbeats();

        // Further scans should not increment
        tracker.scan();
        tracker.scan();
        assertEquals(missedAtDisconnect, node.missedHeartbeats());
        assertEquals(DataPlaneNodeState.DISCONNECTED, node.state());
    }

    // --- Heartbeat resets counter ---

    @Test
    void heartbeatResetsCounterKeepsNodeHealthy() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        node.recordHeartbeat(1, 0, 0, 0); // CONNECTED -> HEALTHY
        createTracker(3, 6);

        // Miss twice
        tracker.scan();
        tracker.scan();
        assertEquals(2, node.missedHeartbeats());
        assertEquals(DataPlaneNodeState.HEALTHY, node.state());

        // Heartbeat arrives, resets counter
        node.recordHeartbeat(2, 0, 0, 0);
        assertEquals(0, node.missedHeartbeats());

        // Miss twice more - still under threshold
        tracker.scan();
        tracker.scan();
        assertEquals(2, node.missedHeartbeats());
        assertEquals(DataPlaneNodeState.HEALTHY, node.state());
    }

    @Test
    void heartbeatRecoveryFromUnhealthy() {
        DataPlaneNode node = registry.register(identity("node-1"), "token-1");
        node.recordHeartbeat(1, 0, 0, 0); // CONNECTED -> HEALTHY
        createTracker(2, 5);

        // Miss twice -> UNHEALTHY
        tracker.scan();
        tracker.scan();
        assertEquals(DataPlaneNodeState.UNHEALTHY, node.state());

        // Heartbeat arrives -> recovers to HEALTHY
        node.recordHeartbeat(2, 0, 0, 0);
        assertEquals(DataPlaneNodeState.HEALTHY, node.state());
        assertEquals(0, node.missedHeartbeats());

        // Continues to stay healthy with heartbeats
        tracker.scan();
        assertEquals(DataPlaneNodeState.HEALTHY, node.state());
    }

    // --- Multiple nodes ---

    @Test
    void scan_handlesMultipleNodesIndependently() {
        DataPlaneNode node1 = registry.register(identity("node-1"), "t1");
        DataPlaneNode node2 = registry.register(identity("node-2"), "t2");
        DataPlaneNode node3 = registry.register(identity("node-3"), "t3");

        node1.recordHeartbeat(1, 0, 0, 0); // HEALTHY
        node2.recordHeartbeat(1, 0, 0, 0); // HEALTHY
        // node3 stays CONNECTED

        createTracker(2, 4);

        // First scan: all active nodes get missed heartbeat incremented
        tracker.scan();
        assertEquals(1, node1.missedHeartbeats());
        assertEquals(1, node2.missedHeartbeats());
        assertEquals(1, node3.missedHeartbeats());

        // node-1 gets heartbeat, others don't
        node1.recordHeartbeat(2, 0, 0, 0);

        // Second scan
        tracker.scan();
        assertEquals(1, node1.missedHeartbeats()); // reset to 0, then +1
        assertEquals(2, node2.missedHeartbeats());  // now UNHEALTHY
        assertEquals(2, node3.missedHeartbeats());   // now UNHEALTHY

        assertEquals(DataPlaneNodeState.HEALTHY, node1.state());
        assertEquals(DataPlaneNodeState.UNHEALTHY, node2.state());
        assertEquals(DataPlaneNodeState.UNHEALTHY, node3.state());
    }

    // --- start / close lifecycle ---

    @Test
    void start_setsRunningToTrue() {
        createTracker(3, 6);

        assertFalse(tracker.isRunning());

        tracker.start();

        assertTrue(tracker.isRunning());
    }

    @Test
    void start_doubleStartThrowsIllegalStateException() {
        createTracker(3, 6);
        tracker.start();

        assertThrows(IllegalStateException.class, () -> tracker.start());
    }

    @Test
    void close_setsRunningToFalse() {
        createTracker(3, 6);
        tracker.start();
        assertTrue(tracker.isRunning());

        tracker.close();

        assertFalse(tracker.isRunning());
    }

    @Test
    void isRunning_falseBeforeStart() {
        createTracker(3, 6);

        assertFalse(tracker.isRunning());
    }

    // --- scan() on empty registry ---

    @Test
    void scan_emptyRegistryDoesNotThrow() {
        createTracker(3, 6);

        // Should not throw
        tracker.scan();
    }
}
