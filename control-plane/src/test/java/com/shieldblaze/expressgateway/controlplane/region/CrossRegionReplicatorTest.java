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
package com.shieldblaze.expressgateway.controlplane.region;

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.conflict.VectorClock;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CopyOnWriteArrayList;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class CrossRegionReplicatorTest {

    private RegionManager regionManager;
    private CrossRegionReplicator replicator;
    private List<CrossRegionReplicator.ReplicationBatch> receivedBatches;

    @BeforeEach
    void setUp() {
        regionManager = new RegionManager("us-east-1");
        regionManager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));

        receivedBatches = new CopyOnWriteArrayList<>();
        replicator = new CrossRegionReplicator(
                "us-east-1",
                regionManager,
                (targetRegion, batch) -> {
                    receivedBatches.add(batch);
                    return CompletableFuture.completedFuture(null);
                },
                100, // 100ms batch window
                100  // max batch size
        );
        replicator.start();
    }

    @AfterEach
    void tearDown() {
        replicator.close();
    }

    @Test
    void enqueuedMutationsAreBatchedAndReplicated() throws Exception {
        ConfigMutation mutation = createMutation("cluster-1");
        VectorClock clock = VectorClock.empty().increment("us-east-1");

        replicator.enqueue(mutation, clock);

        // Wait for batch flush
        Thread.sleep(300);

        assertEquals(1, receivedBatches.size());
        assertEquals("us-east-1", receivedBatches.get(0).sourceRegion());
        assertEquals(1, receivedBatches.get(0).entries().size());
    }

    @Test
    void multipleMutationsBatchedTogether() throws Exception {
        VectorClock clock = VectorClock.empty().increment("us-east-1");
        for (int i = 0; i < 5; i++) {
            replicator.enqueue(createMutation("cluster-" + i), clock);
        }

        Thread.sleep(300);

        // All 5 mutations should be in a single batch
        int totalEntries = receivedBatches.stream()
                .mapToInt(b -> b.entries().size()).sum();
        assertEquals(5, totalEntries);
    }

    @Test
    void noRemoteRegionsReEnqueuesEntries() throws Exception {
        // Remove the remote region so no targets exist
        regionManager.removeRegion("eu-west-1");

        replicator.enqueue(createMutation("cluster-1"), VectorClock.empty().increment("us-east-1"));

        // Poll for the condition: the entry should be continuously re-enqueued.
        // We poll rather than sleeping a fixed amount because pendingCount() can
        // transiently read 0 during the nanosecond window between poll() and
        // addAll() inside flushBatch().
        long deadline = System.currentTimeMillis() + 2000;
        while (replicator.pendingCount() == 0 && System.currentTimeMillis() < deadline) {
            Thread.sleep(20);
        }

        // Entries should be re-enqueued, not discarded
        assertTrue(receivedBatches.isEmpty());
        assertTrue(replicator.pendingCount() > 0, "Entries should be re-enqueued when no targets");
    }

    @Test
    void replicatedCountAccurateWithMultipleTargets() throws Exception {
        // Add a second remote region
        regionManager.updateRegion(new RegionManager.RegionState(
                "ap-south-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));

        CrossRegionReplicator multiTargetReplicator = new CrossRegionReplicator(
                "us-east-1", regionManager,
                (targetRegion, batch) -> {
                    receivedBatches.add(batch);
                    return CompletableFuture.completedFuture(null);
                },
                100, 100
        );
        multiTargetReplicator.start();

        VectorClock clock = VectorClock.empty().increment("us-east-1");
        multiTargetReplicator.enqueue(createMutation("cluster-1"), clock);
        multiTargetReplicator.enqueue(createMutation("cluster-2"), clock);

        Thread.sleep(300);

        // replicatedCount should count per-entry (2), not per-target-per-entry (2*2=4)
        assertEquals(2, multiTargetReplicator.replicatedCount(),
                "replicatedCount should count per-entry, not per-target-per-entry");

        multiTargetReplicator.close();
    }

    @Test
    void failedReplicationsCountCorrectly() throws Exception {
        CrossRegionReplicator failingReplicator = new CrossRegionReplicator(
                "us-east-1", regionManager,
                (targetRegion, batch) -> CompletableFuture.failedFuture(new RuntimeException("network error")),
                100, 100
        );
        failingReplicator.start();

        failingReplicator.enqueue(createMutation("cluster-1"), VectorClock.empty().increment("us-east-1"));
        Thread.sleep(500); // allow time for retries

        assertTrue(failingReplicator.failedCount() > 0);
        failingReplicator.close();
    }

    @Test
    void replicatorMetricsTrackCorrectly() throws Exception {
        VectorClock clock = VectorClock.empty().increment("us-east-1");
        replicator.enqueue(createMutation("cluster-1"), clock);
        replicator.enqueue(createMutation("cluster-2"), clock);

        Thread.sleep(300);

        assertEquals(2, replicator.replicatedCount());
        assertEquals(0, replicator.failedCount());
    }

    @Test
    void enqueueAfterCloseThrowsException() {
        replicator.close();

        assertThrows(IllegalStateException.class, () ->
                replicator.enqueue(createMutation("cluster-1"), VectorClock.empty().increment("us-east-1")));
    }

    private static ConfigMutation createMutation(String name) {
        return new ConfigMutation.Upsert(new ConfigResource(
                new ConfigResourceId("cluster", "global", name),
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1L,
                Instant.now(),
                Instant.now(),
                "test-user",
                Map.of(),
                new ClusterSpec(name, "round-robin", "default-hc", 10000, 30)
        ));
    }
}
