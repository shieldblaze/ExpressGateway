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
package com.shieldblaze.expressgateway.controlplane.resilience;

import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneCluster;
import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneInstance;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.testutil.InMemoryKVStore;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class PartitionHandlerTest {

    private InMemoryKVStore kvStore;
    private ControlPlaneCluster cluster;
    private PartitionHandler handler;
    private List<PartitionHandler.PartitionState> stateTransitions;

    @BeforeEach
    void setUp() throws KVStoreException {
        kvStore = new InMemoryKVStore();
        ControlPlaneConfiguration config = new ControlPlaneConfiguration()
                .grpcBindAddress("127.0.0.1")
                .grpcPort(9443)
                .clusterEnabled(true)
                .region("us-east-1");

        cluster = new ControlPlaneCluster(kvStore, config, "instance-1", "us-east-1");
        cluster.start();

        stateTransitions = new ArrayList<>();
        handler = new PartitionHandler(cluster, 3, Duration.ofMillis(100));
        handler.addListener((prev, curr, reason) -> stateTransitions.add(curr));
    }

    @Test
    void stateTransitionsAreRecordedByFieldListener() {
        // Create a handler with expected 5 nodes but only 1 is present
        PartitionHandler strictHandler = new PartitionHandler(cluster, 5, Duration.ofMillis(50));
        strictHandler.addListener((prev, curr, reason) -> stateTransitions.add(curr));

        strictHandler.evaluate();
        assertFalse(stateTransitions.isEmpty(), "stateTransitions should record listener callbacks");
        assertEquals(PartitionHandler.PartitionState.SUSPECTED, stateTransitions.get(0));
    }

    @Test
    void initialStateIsNormal() {
        assertEquals(PartitionHandler.PartitionState.NORMAL, handler.currentState());
    }

    @Test
    void quorumSizeCalculation() {
        // 3 nodes expected, quorum = 3/2 + 1 = 2
        assertEquals(2, handler.quorumSize());
    }

    @Test
    void singleNodeHasQuorumWithSizeOne() throws KVStoreException {
        PartitionHandler singleHandler = new PartitionHandler(cluster, 1, Duration.ofMillis(100));
        assertTrue(singleHandler.hasQuorum());
    }

    @Test
    void writesAllowedInNormalState() {
        assertTrue(handler.isWriteAllowed());
    }

    @Test
    void evaluateWithQuorumStaysNormal() {
        // With expectedClusterSize=1 and 1 peer, quorum=1 which is satisfied
        PartitionHandler singleHandler = new PartitionHandler(cluster, 1, Duration.ofMillis(100));
        List<PartitionHandler.PartitionState> transitions = new ArrayList<>();
        singleHandler.addListener((prev, curr, reason) -> transitions.add(curr));

        PartitionHandler.PartitionState result = singleHandler.evaluate();
        assertEquals(PartitionHandler.PartitionState.NORMAL, result);
        assertTrue(transitions.isEmpty()); // no change
    }

    @Test
    void stateListenerFiresOnTransition() throws Exception {
        // Create a handler with expected 5 nodes but only 1 is present
        PartitionHandler strictHandler = new PartitionHandler(cluster, 5, Duration.ofMillis(50));
        List<PartitionHandler.PartitionState> transitions = new ArrayList<>();
        strictHandler.addListener((prev, curr, reason) -> transitions.add(curr));

        // With 1 node and quorum = 3, we should go to SUSPECTED
        strictHandler.evaluate();
        assertEquals(1, transitions.size());
        assertEquals(PartitionHandler.PartitionState.SUSPECTED, transitions.get(0));
    }

    @Test
    void suspectedTransitionsAfterGracePeriod() throws Exception {
        PartitionHandler strictHandler = new PartitionHandler(cluster, 5, Duration.ofMillis(50));

        // First evaluate: transitions to SUSPECTED
        strictHandler.evaluate();
        assertEquals(PartitionHandler.PartitionState.SUSPECTED, strictHandler.currentState());

        // Wait for grace period
        Thread.sleep(100);

        // Second evaluate: grace period expired
        PartitionHandler.PartitionState result = strictHandler.evaluate();
        // Since cluster.isLeader() returns true for InMemoryKVStore, it should go to DEGRADED
        assertTrue(result == PartitionHandler.PartitionState.DEGRADED ||
                   result == PartitionHandler.PartitionState.PARTITIONED);
    }

    @Test
    void reachablePeerCountReflectsCluster() {
        assertTrue(handler.reachablePeerCount() >= 1);
    }

    @Test
    void concurrentEvaluateDoesNotResetSuspectedSince() throws Exception {
        // Create a handler that will enter SUSPECTED state
        PartitionHandler strictHandler = new PartitionHandler(cluster, 5, Duration.ofMillis(200));

        // First evaluate: enters SUSPECTED with a suspectedSince timestamp
        strictHandler.evaluate();
        assertEquals(PartitionHandler.PartitionState.SUSPECTED, strictHandler.currentState());

        Instant firstSuspectedSince = strictHandler.currentSnapshot().suspectedSince();
        assertNotNull(firstSuspectedSince, "suspectedSince should be set after entering SUSPECTED");

        // Sleep a bit, then call evaluate again -- suspectedSince should NOT be reset
        Thread.sleep(50);
        strictHandler.evaluate();

        Instant secondSuspectedSince = strictHandler.currentSnapshot().suspectedSince();
        assertEquals(firstSuspectedSince, secondSuspectedSince,
                "suspectedSince must not be reset on subsequent evaluations while still SUSPECTED");
    }

    @Test
    void partitionSnapshotBundlesStateAndSuspectedSince() throws Exception {
        PartitionHandler strictHandler = new PartitionHandler(cluster, 5, Duration.ofMillis(50));

        // Initial snapshot
        PartitionHandler.PartitionSnapshot snapshot = strictHandler.currentSnapshot();
        assertEquals(PartitionHandler.PartitionState.NORMAL, snapshot.state());
        assertEquals(null, snapshot.suspectedSince());

        // Enter SUSPECTED
        strictHandler.evaluate();
        snapshot = strictHandler.currentSnapshot();
        assertEquals(PartitionHandler.PartitionState.SUSPECTED, snapshot.state());
        assertNotNull(snapshot.suspectedSince());
    }

    @Test
    void concurrentEvaluateWithMultipleThreads() throws Exception {
        PartitionHandler strictHandler = new PartitionHandler(cluster, 5, Duration.ofMillis(100));

        int threadCount = 10;
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        ExecutorService executor = Executors.newFixedThreadPool(threadCount);

        for (int i = 0; i < threadCount; i++) {
            executor.submit(() -> {
                try {
                    startLatch.await();
                    strictHandler.evaluate();
                } catch (Exception e) {
                    // ignored
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        startLatch.countDown();
        doneLatch.await(5, TimeUnit.SECONDS);
        executor.shutdown();

        // State should be SUSPECTED (all threads see below-quorum)
        assertEquals(PartitionHandler.PartitionState.SUSPECTED, strictHandler.currentState());

        // suspectedSince should be set and consistent
        assertNotNull(strictHandler.currentSnapshot().suspectedSince());
    }
}
