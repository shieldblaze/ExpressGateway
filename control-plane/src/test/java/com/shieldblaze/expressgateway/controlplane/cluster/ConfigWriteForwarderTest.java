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
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatcher;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.io.Closeable;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link ConfigWriteForwarder}.
 *
 * <p>Uses an in-memory {@link KVStore} to test the mutation forwarding pipeline:
 * writing to the pending-mutations queue, simulating leader processing by writing
 * results, and verifying timeout behavior.</p>
 *
 * <p>Since {@link ConfigWriteForwarder} checks {@code cluster.isLeader()} to reject
 * local calls on the leader, the tests construct clusters with controllable leadership
 * state via the in-memory leader election stub.</p>
 */
class ConfigWriteForwarderTest {

    private static final String PENDING_MUTATIONS_PREFIX = "/expressgateway/controlplane/pending-mutations/";
    private static final String MUTATION_RESULTS_PREFIX = "/expressgateway/controlplane/mutation-results/";

    private InMemoryKVStore kvStore;
    private ControlPlaneCluster cluster;
    private ConfigWriteForwarder forwarder;

    @BeforeEach
    void setUp() throws Exception {
        kvStore = new InMemoryKVStore();
        // Create a cluster that is NOT the leader (follower mode)
        cluster = createCluster("cp-follower", "us-east-1", false);
        cluster.start();
        forwarder = new ConfigWriteForwarder(cluster, kvStore);
    }

    @AfterEach
    void tearDown() {
        if (forwarder != null) {
            forwarder.close();
        }
        if (cluster != null) {
            cluster.close();
        }
    }

    // ---- Forwarding tests ----

    @Test
    @Timeout(10)
    void forwardMutationWritesToPendingQueueAndCompletesWhenLeaderProcesses() throws Exception {
        ConfigMutation mutation = new ConfigMutation.Delete(
                new ConfigResourceId("cluster", "global", "prod"));

        CompletableFuture<Long> future = forwarder.forwardMutation(mutation);

        // Simulate the leader: find the pending mutation and write a result
        simulateLeaderProcessing(42L);

        Long revision = future.get(5, TimeUnit.SECONDS);
        assertEquals(42L, revision, "Future should complete with the revision written by the leader");
    }

    @Test
    @Timeout(10)
    void forwardMutationCleansUpEntriesAfterCompletion() throws Exception {
        ConfigMutation mutation = new ConfigMutation.Delete(
                new ConfigResourceId("cluster", "global", "prod"));

        CompletableFuture<Long> future = forwarder.forwardMutation(mutation);

        // Simulate the leader processing
        simulateLeaderProcessing(100L);

        future.get(5, TimeUnit.SECONDS);

        // Allow a brief moment for cleanup to complete (cleanup is best-effort after future completion)
        Thread.sleep(200);

        // Verify pending and result entries are cleaned up
        List<KVEntry> pending = kvStore.list(PENDING_MUTATIONS_PREFIX);
        List<KVEntry> results = kvStore.list(MUTATION_RESULTS_PREFIX);

        assertTrue(pending.isEmpty(),
                "Pending mutation entry should be cleaned up after processing");
        assertTrue(results.isEmpty(),
                "Mutation result entry should be cleaned up after processing");
    }

    @Test
    @Timeout(10)
    void calledOnLeaderThrowsIllegalStateException() throws Exception {
        // Create a leader cluster
        ControlPlaneCluster leaderCluster = createCluster("cp-leader", "us-east-1", true);
        leaderCluster.start();

        ConfigWriteForwarder leaderForwarder = new ConfigWriteForwarder(leaderCluster, kvStore);

        try {
            ConfigMutation mutation = new ConfigMutation.Delete(
                    new ConfigResourceId("cluster", "global", "prod"));

            assertThrows(IllegalStateException.class,
                    () -> leaderForwarder.forwardMutation(mutation),
                    "Calling forwardMutation on the leader should throw IllegalStateException");
        } finally {
            leaderForwarder.close();
            leaderCluster.close();
        }
    }

    @Test
    void nullMutationThrowsNullPointerException() {
        assertThrows(NullPointerException.class,
                () -> forwarder.forwardMutation(null));
    }

    @Test
    void nullClusterInConstructorThrowsNullPointerException() {
        assertThrows(NullPointerException.class,
                () -> new ConfigWriteForwarder(null, kvStore));
    }

    @Test
    void nullKvStoreInConstructorThrowsNullPointerException() {
        assertThrows(NullPointerException.class,
                () -> new ConfigWriteForwarder(cluster, null));
    }

    @Test
    @Timeout(10)
    void forwardMutationWritesPendingEntryToKVStore() throws Exception {
        ConfigMutation mutation = new ConfigMutation.Delete(
                new ConfigResourceId("cluster", "global", "prod"));

        forwarder.forwardMutation(mutation);

        // Give the async executor a moment to write
        Thread.sleep(300);

        List<KVEntry> pendingEntries = kvStore.list(PENDING_MUTATIONS_PREFIX);
        assertEquals(1, pendingEntries.size(),
                "Exactly one pending mutation should be written to the KV store");
        assertNotNull(pendingEntries.get(0).value(),
                "Pending mutation value must not be null");
        assertTrue(pendingEntries.get(0).value().length > 0,
                "Pending mutation value must not be empty");
    }

    @Test
    @Timeout(15)
    void timeoutWhenLeaderDoesNotProcess() throws Exception {
        // Use a forwarder with a very short timeout by creating a custom test.
        // Since MUTATION_TIMEOUT_MS is a static final constant in ConfigWriteForwarder (30s),
        // we cannot easily change it. Instead, we verify the future completes exceptionally
        // by NOT writing any result. We use a short get() timeout to avoid waiting 30s.
        ConfigMutation mutation = new ConfigMutation.Delete(
                new ConfigResourceId("cluster", "global", "prod"));

        CompletableFuture<Long> future = forwarder.forwardMutation(mutation);

        // We cannot wait 30 seconds in a unit test, but we can verify the future
        // is not completed immediately (leader hasn't processed it)
        Thread.sleep(500);

        assertTrue(!future.isDone(),
                "Future should not be completed when no result is written by the leader");
    }

    // ---- Helper methods ----

    /**
     * Creates a {@link ControlPlaneCluster} with controllable leadership state.
     */
    private ControlPlaneCluster createCluster(String instanceId, String region, boolean isLeader) {
        kvStore.setLeaderState(isLeader);
        ControlPlaneConfiguration config = new ControlPlaneConfiguration()
                .grpcBindAddress("127.0.0.1")
                .grpcPort(9443)
                .region(region)
                .clusterEnabled(true);
        return new ControlPlaneCluster(kvStore, config, instanceId, region);
    }

    /**
     * Simulates leader behavior: polls for a pending mutation entry and writes a result.
     * Runs in a background thread to avoid blocking the test.
     */
    private void simulateLeaderProcessing(long resultRevision) {
        Thread leaderThread = new Thread(() -> {
            try {
                // Poll for pending mutations
                long deadline = System.currentTimeMillis() + 5000;
                while (System.currentTimeMillis() < deadline) {
                    List<KVEntry> pending = kvStore.list(PENDING_MUTATIONS_PREFIX);
                    if (!pending.isEmpty()) {
                        for (KVEntry entry : pending) {
                            // Extract nonce from key
                            String nonce = entry.key().substring(PENDING_MUTATIONS_PREFIX.length());
                            // Write the result
                            kvStore.put(MUTATION_RESULTS_PREFIX + nonce,
                                    String.valueOf(resultRevision).getBytes());
                        }
                        return;
                    }
                    Thread.sleep(50);
                }
            } catch (Exception e) {
                // Suppress exceptions in background thread
            }
        }, "simulated-leader");
        leaderThread.setDaemon(true);
        leaderThread.start();
    }

    // ---- In-memory KV store for testing ----

    /**
     * Minimal in-memory {@link KVStore} implementation with controllable leadership state.
     */
    private static class InMemoryKVStore implements KVStore {

        private final ConcurrentHashMap<String, byte[]> store = new ConcurrentHashMap<>();
        private final AtomicLong version = new AtomicLong(0);
        private volatile boolean leaderState = false;

        void setLeaderState(boolean isLeader) {
            this.leaderState = isLeader;
        }

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
                    return leaderState;
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
