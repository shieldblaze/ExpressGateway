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

import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.testutil.InMemoryKVStore;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link PersistentNodeRegistry} using an {@link InMemoryKVStore}.
 *
 * <p>Tests verify:</p>
 * <ul>
 *   <li>Register persists to KV store</li>
 *   <li>Deregister removes from KV store</li>
 *   <li>Crash recovery via loadFromKVStore()</li>
 *   <li>Coalescing buffer for heartbeat version updates</li>
 * </ul>
 */
class PersistentNodeRegistryTest {

    private InMemoryKVStore kvStore;
    private NodeRegistry delegate;
    private PersistentNodeRegistry registry;

    @BeforeEach
    void setUp() {
        kvStore = new InMemoryKVStore();
        delegate = new NodeRegistry();
        registry = new PersistentNodeRegistry(delegate, kvStore, 200); // Fast flush for tests
        registry.start();
    }

    @AfterEach
    void tearDown() {
        if (registry != null) {
            registry.close();
        }
    }

    private NodeIdentity identity(String nodeId) {
        return NodeIdentity.newBuilder()
                .setNodeId(nodeId)
                .setClusterId("test-cluster")
                .setEnvironment("test")
                .setAddress("127.0.0.1")
                .setBuildVersion("1.0.0")
                .build();
    }

    // ===========================================================================
    // Test: Register persists to KV store
    // ===========================================================================

    @Test
    @Timeout(10)
    @DisplayName("Registered node is persisted to KV store")
    void testRegisterPersistsToKVStore() throws KVStoreException {
        DataPlaneNode node = registry.register(identity("node-1"), "session-1");
        assertNotNull(node, "register() should return a non-null node");

        // Verify persisted in KV store
        var entry = kvStore.get("/expressgateway/nodes/node-1");
        assertTrue(entry.isPresent(), "Node should be persisted in KV store after register()");

        // Verify in-memory delegate also has the node
        assertTrue(delegate.get("node-1").isPresent(), "Node should be in the in-memory registry");
    }

    // ===========================================================================
    // Test: Deregister removes from KV store
    // ===========================================================================

    @Test
    @Timeout(10)
    @DisplayName("Deregistered node is removed from KV store")
    void testDeregisterRemovesFromKVStore() throws KVStoreException {
        registry.register(identity("node-2"), "session-2");

        // Verify it's there
        assertTrue(kvStore.get("/expressgateway/nodes/node-2").isPresent());

        // Deregister
        DataPlaneNode removed = registry.deregister("node-2");
        assertNotNull(removed, "deregister() should return the removed node");

        // Verify removed from KV store
        assertTrue(kvStore.get("/expressgateway/nodes/node-2").isEmpty(),
                "Node should be removed from KV store after deregister()");

        // Verify removed from in-memory delegate
        assertTrue(delegate.get("node-2").isEmpty(), "Node should be removed from in-memory registry");
    }

    // ===========================================================================
    // Test: Deregister non-existent node returns null
    // ===========================================================================

    @Test
    @Timeout(10)
    @DisplayName("Deregister non-existent node returns null")
    void testDeregisterNonExistentReturnsNull() {
        DataPlaneNode removed = registry.deregister("no-such-node");
        assertNull(removed, "deregister() should return null for non-existent node");
    }

    // ===========================================================================
    // Test: Crash recovery via loadFromKVStore()
    // ===========================================================================

    @Test
    @Timeout(10)
    @DisplayName("loadFromKVStore() recovers persisted nodes into a fresh registry")
    void testCrashRecovery() throws KVStoreException {
        // Register 3 nodes
        registry.register(identity("recover-1"), "session-r1");
        registry.register(identity("recover-2"), "session-r2");
        registry.register(identity("recover-3"), "session-r3");

        // Simulate CP crash: create a new registry backed by the same KV store
        registry.close();

        NodeRegistry freshDelegate = new NodeRegistry();
        PersistentNodeRegistry recoveredRegistry = new PersistentNodeRegistry(freshDelegate, kvStore, 200);

        try {
            int loadedCount = recoveredRegistry.loadFromKVStore();
            assertEquals(3, loadedCount, "Should recover 3 nodes from KV store");

            // Verify all nodes are in the fresh delegate
            assertTrue(freshDelegate.get("recover-1").isPresent(), "recover-1 should be restored");
            assertTrue(freshDelegate.get("recover-2").isPresent(), "recover-2 should be restored");
            assertTrue(freshDelegate.get("recover-3").isPresent(), "recover-3 should be restored");
        } finally {
            recoveredRegistry.close();
        }
    }

    // ===========================================================================
    // Test: Recovery skips already-registered nodes
    // ===========================================================================

    @Test
    @Timeout(10)
    @DisplayName("loadFromKVStore() skips nodes that are already registered in the delegate")
    void testRecoverySkipsDuplicates() throws KVStoreException {
        registry.register(identity("dup-node"), "session-dup");
        registry.close();

        // Pre-register the same node in the fresh delegate
        NodeRegistry freshDelegate = new NodeRegistry();
        freshDelegate.register(identity("dup-node"), "session-dup-2");

        PersistentNodeRegistry recoveredRegistry = new PersistentNodeRegistry(freshDelegate, kvStore, 200);

        try {
            int loadedCount = recoveredRegistry.loadFromKVStore();
            assertEquals(0, loadedCount, "Should skip already-registered nodes");

            // The existing registration should be untouched
            DataPlaneNode node = freshDelegate.get("dup-node").orElseThrow();
            assertEquals("session-dup-2", node.sessionToken(),
                    "Existing node should keep its original session token");
        } finally {
            recoveredRegistry.close();
        }
    }

    // ===========================================================================
    // Test: Coalescing buffer for heartbeat version updates
    // ===========================================================================

    @Test
    @Timeout(15)
    @DisplayName("markDirty() coalesces multiple heartbeat updates into a single KV write")
    void testCoalescingBuffer() throws Exception {
        DataPlaneNode node = registry.register(identity("coalesce-node"), "session-coal");

        // Update the node's config version and mark dirty multiple times
        node.recordHeartbeat(1, 10, 0.2, 0.3);
        registry.markDirty("coalesce-node");

        node.recordHeartbeat(2, 20, 0.4, 0.5);
        registry.markDirty("coalesce-node");

        node.recordHeartbeat(3, 30, 0.6, 0.7);
        registry.markDirty("coalesce-node");

        // Wait for the flush cycle to run (flush interval is 200ms)
        Thread.sleep(500);

        // Verify the KV store has the latest state (version 3, not 1 or 2)
        var entry = kvStore.get("/expressgateway/nodes/coalesce-node");
        assertTrue(entry.isPresent(), "Node should be in KV store");

        // Parse the persisted record to verify the version
        String json = new String(entry.get().value());
        assertTrue(json.contains("\"appliedConfigVersion\":3"),
                "KV store should have the latest version (3), got: " + json);
    }

    // ===========================================================================
    // Test: Multiple nodes registered and recovered
    // ===========================================================================

    @Test
    @Timeout(10)
    @DisplayName("Multiple nodes can be registered and recovered independently")
    void testMultipleNodesIndependent() throws KVStoreException {
        registry.register(identity("multi-1"), "sess-m1");
        registry.register(identity("multi-2"), "sess-m2");

        // Deregister one
        registry.deregister("multi-1");

        // Only multi-2 should be in KV store
        assertTrue(kvStore.get("/expressgateway/nodes/multi-1").isEmpty(),
                "multi-1 should be removed from KV store");
        assertTrue(kvStore.get("/expressgateway/nodes/multi-2").isPresent(),
                "multi-2 should still be in KV store");

        // Recovery should only restore multi-2
        registry.close();
        NodeRegistry freshDelegate = new NodeRegistry();
        PersistentNodeRegistry recoveredRegistry = new PersistentNodeRegistry(freshDelegate, kvStore, 200);

        try {
            int count = recoveredRegistry.loadFromKVStore();
            assertEquals(1, count, "Should only recover multi-2");
            assertTrue(freshDelegate.get("multi-2").isPresent());
            assertTrue(freshDelegate.get("multi-1").isEmpty());
        } finally {
            recoveredRegistry.close();
        }
    }

    // ===========================================================================
    // Test: Constructor validation
    // ===========================================================================

    @Test
    @DisplayName("Constructor rejects flushIntervalMs < 100")
    void testInvalidFlushInterval() {
        org.junit.jupiter.api.Assertions.assertThrows(IllegalArgumentException.class,
                () -> new PersistentNodeRegistry(new NodeRegistry(), kvStore, 50));
    }
}
