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

import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneServer;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.StorageConfiguration;
import com.shieldblaze.expressgateway.controlplane.registry.PersistentNodeRegistry;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Tag;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestInstance;
import org.junit.jupiter.api.TestMethodOrder;
import org.junit.jupiter.api.Timeout;

import java.io.IOException;
import java.net.ServerSocket;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.UUID;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Abstract multi-instance control plane integration test.
 *
 * <p>Starts 3 ControlPlaneServer instances connected to the SAME KV store backend
 * and verifies leader election, failover, peer discovery, and cross-instance
 * node registration visibility.</p>
 *
 * <p>Each subclass provides a concrete KVStore backed by a Testcontainer.</p>
 */
@Tag("integration")
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
public abstract class AbstractMultiInstanceTest {

    private static final String[] REGIONS = {"us-east-1", "eu-west-1", "ap-south-1"};

    protected final List<ControlPlaneServer> servers = new ArrayList<>();
    protected final List<KVStore> kvStores = new ArrayList<>();

    /**
     * Create a new KVStore instance connected to the shared backend container.
     * Each call must return a fresh KVStore client (not the same instance).
     */
    protected abstract KVStore createKVStore() throws Exception;

    /**
     * Returns the KvStoreType for the backend under test.
     */
    protected abstract ControlPlaneConfiguration.KvStoreType kvStoreType();

    /**
     * Finds an available local TCP port by briefly opening and closing a server socket.
     */
    private static int findAvailablePort() throws IOException {
        try (ServerSocket ss = new ServerSocket(0)) {
            ss.setReuseAddress(true);
            return ss.getLocalPort();
        }
    }

    /**
     * Hook for subclasses to set up their KV store backend before any servers are created.
     * Called before {@link #startCluster()}.
     */
    protected void setUpBackend() throws Exception {
        // Default no-op; subclasses override as needed (e.g., ZK Curator injection)
    }

    @BeforeAll
    void startCluster() throws Exception {
        setUpBackend();
        for (int i = 0; i < 3; i++) {
            KVStore kvStore = createKVStore();
            kvStores.add(kvStore);

            int port = findAvailablePort();

            StorageConfiguration storageConfig = new StorageConfiguration()
                    .endpoints(List.of("dummy://localhost:0"));

            ControlPlaneConfiguration config = new ControlPlaneConfiguration()
                    .grpcPort(port)
                    .heartbeatIntervalMs(5000)
                    .heartbeatMissThreshold(3)
                    .heartbeatDisconnectThreshold(6)
                    .heartbeatScanIntervalMs(2000)
                    .writeBatchWindowMs(100)
                    .maxJournalLag(100)
                    .kvStoreType(kvStoreType())
                    .maxRequestsPerSecondPerNode(1000)
                    .clusterEnabled(true)
                    .region(REGIONS[i])
                    .reconnectBurst(500)
                    .reconnectRefillRate(100)
                    .storage(storageConfig)
                    .validate();

            ControlPlaneServer server = new ControlPlaneServer(config, kvStore);
            server.start();
            servers.add(server);
        }

        // Allow cluster to stabilize: peer discovery, leader election, heartbeat registration.
        Thread.sleep(5000);
    }

    @AfterAll
    void stopCluster() throws Exception {
        for (ControlPlaneServer server : servers) {
            try {
                server.close();
            } catch (Exception e) {
                // Best effort
            }
        }
        for (KVStore kvStore : kvStores) {
            try {
                kvStore.close();
            } catch (Exception e) {
                // Best effort
            }
        }
        servers.clear();
        kvStores.clear();
    }

    // ---- Helpers ----

    private boolean awaitCondition(long timeoutMs, BooleanCondition condition) throws Exception {
        long deadline = System.currentTimeMillis() + timeoutMs;
        while (System.currentTimeMillis() < deadline) {
            try {
                if (condition.check()) {
                    return true;
                }
            } catch (Exception e) {
                // Swallow and retry until timeout
            }
            Thread.sleep(200);
        }
        return condition.check();
    }

    @FunctionalInterface
    private interface BooleanCondition {
        boolean check() throws Exception;
    }

    // ===========================================================================
    // Test: Leader Election -- exactly one leader
    // ===========================================================================

    @Test
    @Order(1)
    @Timeout(60)
    @DisplayName("Exactly one leader is elected among 3 instances")
    void testExactlyOneLeaderElected() throws Exception {
        boolean converged = awaitCondition(30_000, () -> {
            long leaderCount = servers.stream().filter(ControlPlaneServer::isLeader).count();
            return leaderCount == 1;
        });
        assertTrue(converged, "Expected exactly 1 leader. Leaders: " +
                servers.stream().filter(ControlPlaneServer::isLeader).count());
    }

    // ===========================================================================
    // Test: Leader Failover
    // ===========================================================================

    @Test
    @Order(100) // Run last -- this test closes a server, affecting other tests
    @Timeout(90)
    @DisplayName("New leader elected within 30s after leader instance is closed")
    void testLeaderFailover() throws Exception {
        // Wait for initial leader
        boolean hasLeader = awaitCondition(30_000, () ->
                servers.stream().anyMatch(ControlPlaneServer::isLeader));
        assertTrue(hasLeader, "Should have a leader before failover test");

        // Find and close the leader
        ControlPlaneServer leader = servers.stream()
                .filter(ControlPlaneServer::isLeader)
                .findFirst()
                .orElseThrow();
        leader.close();

        // Remaining servers should elect a new leader within 30s
        List<ControlPlaneServer> remaining = servers.stream()
                .filter(s -> s != leader)
                .toList();

        boolean newLeaderElected = awaitCondition(30_000, () ->
                remaining.stream().anyMatch(ControlPlaneServer::isLeader));
        assertTrue(newLeaderElected, "A new leader should be elected after the old leader is closed");

        long leaderCount = remaining.stream().filter(ControlPlaneServer::isLeader).count();
        assertEquals(1, leaderCount, "Exactly one new leader should be elected");
    }

    // ===========================================================================
    // Test: Peer Discovery
    // ===========================================================================

    @Test
    @Order(2)
    @Timeout(60)
    @DisplayName("All 3 instances see each other as peers")
    void testPeerDiscovery() throws Exception {
        boolean allPeersVisible = awaitCondition(30_000, () -> {
            for (ControlPlaneServer server : servers) {
                if (!server.isClusterEnabled()) return false;
                Collection<ControlPlaneInstance> peers = server.clusterPeers();
                if (peers.size() < 3) return false;
            }
            return true;
        });
        assertTrue(allPeersVisible,
                "All 3 instances should see each other as peers. Peer counts: " +
                        servers.stream().map(s -> String.valueOf(s.clusterPeers().size())).toList());
    }

    // ===========================================================================
    // Test: Node Registration Visible Across Instances
    // ===========================================================================

    @Test
    @Order(3)
    @Timeout(60)
    @DisplayName("Node registered on one instance is visible via KV store on another")
    void testCrossInstanceNodeVisibility() throws Exception {
        String nodeId = "multi-inst-node-" + UUID.randomUUID().toString().substring(0, 8);

        // Register a node through a PersistentNodeRegistry on the first server
        ControlPlaneServer server0 = servers.get(0);
        PersistentNodeRegistry persistentRegistry = new PersistentNodeRegistry(
                server0.nodeRegistry(), kvStores.get(0));
        persistentRegistry.start();

        try {
            NodeIdentity identity = NodeIdentity.newBuilder()
                    .setNodeId(nodeId)
                    .setClusterId("test-cluster")
                    .setEnvironment("test")
                    .setAddress("127.0.0.1")
                    .setBuildVersion("1.0.0")
                    .build();
            persistentRegistry.register(identity, "session-" + nodeId);

            // The node should be retrievable from the KV store by another server's KV client
            boolean visible = awaitCondition(10_000, () -> {
                var entry = kvStores.get(1).get("/expressgateway/nodes/" + nodeId);
                return entry.isPresent();
            });
            assertTrue(visible, "Node registered on server[0] should be visible via kvStore[1]");

        } finally {
            persistentRegistry.close();
            server0.nodeRegistry().deregister(nodeId);
        }
    }
}
