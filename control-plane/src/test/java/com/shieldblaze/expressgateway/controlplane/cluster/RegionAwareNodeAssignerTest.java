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

import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatcher;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.io.Closeable;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link RegionAwareNodeAssigner}.
 *
 * <p>Uses an in-memory {@link KVStore} implementation to construct a real
 * {@link ControlPlaneCluster} with pre-populated peers. This avoids mocking
 * the final cluster class and exercises the real peer discovery path.</p>
 *
 * <p>Test scenarios cover the three-step assignment strategy: same-region current
 * instance (no reassignment), healthiest same-region peer suggestion, and
 * fallback to empty when no same-region peer exists.</p>
 */
class RegionAwareNodeAssignerTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final String INSTANCES_PREFIX = "/expressgateway/controlplane/instances/";

    private InMemoryKVStore kvStore;
    private ControlPlaneCluster cluster;

    @BeforeEach
    void setUp() {
        kvStore = new InMemoryKVStore();
    }

    @AfterEach
    void tearDown() {
        if (cluster != null) {
            cluster.close();
        }
    }

    // ---- Assignment strategy tests ----

    @Test
    void currentInstanceInSameRegionAsNodeReturnsEmpty() throws Exception {
        // Self (cp-self) is in us-east-1. Node is also in us-east-1.
        // No reassignment needed.
        cluster = createCluster("cp-self", "us-east-1");

        // Add a peer in a different region
        putPeer("cp-eu", "eu-west-1", "10.0.1.1", 9443, 1000, 2000);

        cluster.start();

        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        Optional<ControlPlaneInstance> suggestion =
                assigner.suggestInstance("us-east-1", "cp-self");

        assertTrue(suggestion.isEmpty(),
                "Should not reassign when current instance is already in the same region as the node");
    }

    @Test
    void nodeInDifferentRegionWithSameRegionPeerSuggestsThatPeer() throws Exception {
        // Self (cp-self) is in us-east-1. Node is in eu-west-1.
        // Peer cp-eu is in eu-west-1 -> should be suggested.
        cluster = createCluster("cp-self", "us-east-1");

        long now = System.currentTimeMillis();
        putPeer("cp-eu", "eu-west-1", "10.0.1.1", 9443, now, now);

        cluster.start();

        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        Optional<ControlPlaneInstance> suggestion =
                assigner.suggestInstance("eu-west-1", "cp-self");

        assertTrue(suggestion.isPresent(),
                "Should suggest a same-region peer when current instance is in a different region");
        assertEquals("cp-eu", suggestion.get().instanceId());
    }

    @Test
    void noSameRegionPeerReturnsEmpty() throws Exception {
        // Self (cp-self) is in us-east-1. Node is in ap-southeast-1.
        // No peer is in ap-southeast-1 -> empty.
        cluster = createCluster("cp-self", "us-east-1");

        putPeer("cp-eu", "eu-west-1", "10.0.1.1", 9443, 1000, 2000);

        cluster.start();

        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        Optional<ControlPlaneInstance> suggestion =
                assigner.suggestInstance("ap-southeast-1", "cp-self");

        assertTrue(suggestion.isEmpty(),
                "Should return empty when no same-region peer exists");
    }

    @Test
    void multipleSameRegionPeersSuggestsHealthiestByHeartbeat() throws Exception {
        // Self (cp-self) is in us-east-1. Node is in eu-west-1.
        // Two peers in eu-west-1: cp-eu-old (heartbeat=1000) and cp-eu-new (heartbeat=5000).
        // Should suggest cp-eu-new (most recent heartbeat).
        cluster = createCluster("cp-self", "us-east-1");

        long baseTime = System.currentTimeMillis();
        putPeer("cp-eu-old", "eu-west-1", "10.0.1.1", 9443, baseTime, baseTime);
        putPeer("cp-eu-new", "eu-west-1", "10.0.1.2", 9443, baseTime, baseTime + 5000);

        cluster.start();

        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        Optional<ControlPlaneInstance> suggestion =
                assigner.suggestInstance("eu-west-1", "cp-self");

        assertTrue(suggestion.isPresent(),
                "Should suggest the healthiest same-region peer");
        assertEquals("cp-eu-new", suggestion.get().instanceId(),
                "Should pick the peer with the most recent heartbeat");
    }

    @Test
    void suggestedPeerIsSameAsCurrentReturnsEmpty() throws Exception {
        // Self (cp-self) is in us-east-1. Node is in eu-west-1 and currently
        // connected to cp-eu. cp-eu is the only peer in eu-west-1.
        // suggestInstance should return empty because the suggestion equals the current.
        cluster = createCluster("cp-self", "us-east-1");

        long now = System.currentTimeMillis();
        putPeer("cp-eu", "eu-west-1", "10.0.1.1", 9443, now, now);

        cluster.start();

        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        Optional<ControlPlaneInstance> suggestion =
                assigner.suggestInstance("eu-west-1", "cp-eu");

        assertTrue(suggestion.isEmpty(),
                "Should return empty when the best same-region peer is already the current instance");
    }

    @Test
    void currentInstanceNotFoundInPeersButSameRegionPeerExists() throws Exception {
        // Self (cp-self) is in us-east-1. The currentInstanceId passed is "cp-unknown"
        // which is not in the peer map. Node is in eu-west-1.
        // Since findPeer("cp-unknown") returns null, the null-check fails gracefully,
        // and it proceeds to find a same-region peer.
        cluster = createCluster("cp-self", "us-east-1");

        long now = System.currentTimeMillis();
        putPeer("cp-eu", "eu-west-1", "10.0.1.1", 9443, now, now);

        cluster.start();

        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        Optional<ControlPlaneInstance> suggestion =
                assigner.suggestInstance("eu-west-1", "cp-unknown");

        assertTrue(suggestion.isPresent(),
                "Should suggest a same-region peer when current instance is unknown");
        assertEquals("cp-eu", suggestion.get().instanceId());
    }

    // ---- Null/blank argument validation ----

    @Test
    void nullClusterThrowsNullPointerException() {
        assertThrows(NullPointerException.class, () -> new RegionAwareNodeAssigner(null));
    }

    @Test
    void nullNodeRegionThrowsNullPointerException() throws Exception {
        cluster = createCluster("cp-self", "us-east-1");
        cluster.start();
        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        assertThrows(NullPointerException.class,
                () -> assigner.suggestInstance(null, "cp-self"));
    }

    @Test
    void nullCurrentInstanceIdThrowsNullPointerException() throws Exception {
        cluster = createCluster("cp-self", "us-east-1");
        cluster.start();
        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        assertThrows(NullPointerException.class,
                () -> assigner.suggestInstance("us-east-1", null));
    }

    @Test
    void blankNodeRegionThrowsIllegalArgumentException() throws Exception {
        cluster = createCluster("cp-self", "us-east-1");
        cluster.start();
        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        assertThrows(IllegalArgumentException.class,
                () -> assigner.suggestInstance("", "cp-self"));

        assertThrows(IllegalArgumentException.class,
                () -> assigner.suggestInstance("   ", "cp-self"));
    }

    @Test
    void blankCurrentInstanceIdThrowsIllegalArgumentException() throws Exception {
        cluster = createCluster("cp-self", "us-east-1");
        cluster.start();
        RegionAwareNodeAssigner assigner = new RegionAwareNodeAssigner(cluster);

        assertThrows(IllegalArgumentException.class,
                () -> assigner.suggestInstance("us-east-1", ""));

        assertThrows(IllegalArgumentException.class,
                () -> assigner.suggestInstance("us-east-1", "   "));
    }

    // ---- Helper methods ----

    /**
     * Creates a {@link ControlPlaneCluster} backed by the in-memory KV store.
     */
    private ControlPlaneCluster createCluster(String instanceId, String region) {
        ControlPlaneConfiguration config = new ControlPlaneConfiguration()
                .grpcBindAddress("127.0.0.1")
                .grpcPort(9443)
                .region(region)
                .clusterEnabled(true);
        return new ControlPlaneCluster(kvStore, config, instanceId, region);
    }

    /**
     * Pre-populates a peer instance in the KV store before the cluster starts.
     * The cluster's {@code loadExistingPeers()} will discover it on startup.
     */
    private void putPeer(String instanceId, String region, String grpcAddress,
                         int grpcPort, long registeredAt, long lastHeartbeat) throws Exception {
        ControlPlaneInstance peer = new ControlPlaneInstance(
                instanceId, region, grpcAddress, grpcPort, registeredAt, lastHeartbeat);
        byte[] serialized = MAPPER.writeValueAsBytes(peer);
        kvStore.put(INSTANCES_PREFIX + instanceId, serialized);
    }

    // ---- In-memory KV store for testing ----

    /**
     * Minimal in-memory {@link KVStore} implementation sufficient for
     * {@link ControlPlaneCluster} construction and peer discovery.
     */
    private static class InMemoryKVStore implements KVStore {

        private final ConcurrentHashMap<String, byte[]> store = new ConcurrentHashMap<>();
        private final AtomicLong version = new AtomicLong(0);

        @Override
        public Optional<KVEntry> get(String key) {
            byte[] v = store.get(key);
            return v == null
                    ? Optional.empty()
                    : Optional.of(new KVEntry(key, v, version.get(), 1));
        }

        @Override
        public List<KVEntry> list(String prefix) {
            String normalizedPrefix = prefix.endsWith("/") ? prefix : prefix + "/";
            return store.entrySet().stream()
                    .filter(e -> e.getKey().startsWith(normalizedPrefix))
                    .sorted(Map.Entry.comparingByKey())
                    .map(e -> new KVEntry(e.getKey(), e.getValue(), version.get(), 1))
                    .toList();
        }

        @Override
        public List<KVEntry> listRecursive(String prefix) {
            return list(prefix);
        }

        @Override
        public long put(String key, byte[] value) {
            store.put(key, value);
            return version.incrementAndGet();
        }

        @Override
        public long cas(String key, byte[] value, long expectedVersion) {
            store.put(key, value);
            return version.incrementAndGet();
        }

        @Override
        public boolean delete(String key) {
            return store.remove(key) != null;
        }

        @Override
        public void deleteTree(String key) {
            store.keySet().removeIf(k -> k.startsWith(key));
        }

        @Override
        public Closeable watch(String keyOrPrefix, KVWatcher watcher) {
            return () -> { };
        }

        @Override
        public Closeable acquireLock(String lockPath) {
            return () -> { };
        }

        @Override
        public LeaderElection leaderElection(String electionPath, String participantId) {
            return new LeaderElection() {
                @Override
                public boolean isLeader() {
                    return true;
                }

                @Override
                public String currentLeaderId() {
                    return participantId;
                }

                @Override
                public void addListener(LeaderChangeListener listener) {
                    // no-op for tests
                }

                @Override
                public void close() {
                    // no-op for tests
                }
            };
        }

        @Override
        public void close() {
            // no-op
        }
    }
}
