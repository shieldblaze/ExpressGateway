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
package com.shieldblaze.expressgateway.controlplane.distribution;

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class AtomicRolloutTest {

    @Test
    void allNodesAcceptCommitSucceeds() {
        AtomicRollout rollout = new AtomicRollout(new AcceptingCallback(), Duration.ofSeconds(5));

        ConfigDelta delta = createDelta();
        List<DataPlaneNode> targets = List.of(createNode("node-1"), createNode("node-2"));

        AtomicRollout.RolloutResult result = rollout.execute(delta, targets);

        assertEquals(AtomicRollout.Phase.COMMITTED, result.phase());
        assertTrue(result.isCommitted());
        assertEquals(2, result.nodeStatus().size());
        assertTrue(result.nodeStatus().values().stream()
                .allMatch(s -> s == AtomicRollout.AckStatus.COMMIT_ACK));
    }

    @Test
    void oneNodeRejectsPrepareAborts() {
        Set<String> rejectNodes = Set.of("node-2");
        AtomicRollout rollout = new AtomicRollout(
                new SelectiveRejectCallback(rejectNodes), Duration.ofSeconds(5));

        ConfigDelta delta = createDelta();
        List<DataPlaneNode> targets = List.of(createNode("node-1"), createNode("node-2"));

        AtomicRollout.RolloutResult result = rollout.execute(delta, targets);

        assertEquals(AtomicRollout.Phase.ABORTED, result.phase());
        assertEquals(AtomicRollout.AckStatus.PREPARE_ACK, result.nodeStatus().get("node-1"));
        assertEquals(AtomicRollout.AckStatus.PREPARE_NACK, result.nodeStatus().get("node-2"));
    }

    @Test
    void emptyTargetListReturnsAborted() {
        AtomicRollout rollout = new AtomicRollout(new AcceptingCallback(), Duration.ofSeconds(5));

        AtomicRollout.RolloutResult result = rollout.execute(createDelta(), List.of());

        assertEquals(AtomicRollout.Phase.ABORTED, result.phase());
        assertTrue(result.nodeStatus().isEmpty());
    }

    @Test
    void timeoutCausesAbort() {
        // Callback that blocks forever on prepare for node-2
        AtomicRollout rollout = new AtomicRollout(new AtomicRollout.RolloutCallback() {
            @Override
            public boolean prepare(DataPlaneNode node, ConfigDelta delta) {
                if ("node-2".equals(node.nodeId())) {
                    try { Thread.sleep(10_000); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                }
                return true;
            }
            @Override
            public boolean commit(DataPlaneNode node, ConfigDelta delta) { return true; }
            @Override
            public boolean abort(DataPlaneNode node, ConfigDelta delta) { return true; }
        }, Duration.ofMillis(200));

        List<DataPlaneNode> targets = List.of(createNode("node-1"), createNode("node-2"));
        AtomicRollout.RolloutResult result = rollout.execute(createDelta(), targets);

        assertEquals(AtomicRollout.Phase.ABORTED, result.phase());
        assertEquals(AtomicRollout.AckStatus.TIMEOUT, result.nodeStatus().get("node-2"));
    }

    @Test
    void rolloutDurationTracked() {
        AtomicRollout rollout = new AtomicRollout(new AcceptingCallback(), Duration.ofSeconds(5));
        AtomicRollout.RolloutResult result = rollout.execute(createDelta(), List.of(createNode("node-1")));

        assertNotNull(result.startTime());
        assertNotNull(result.endTime());
        assertTrue(result.duration().toMillis() >= 0);
    }

    @Test
    void prepareExceptionTreatedAsNack() {
        AtomicRollout rollout = new AtomicRollout(new AtomicRollout.RolloutCallback() {
            @Override
            public boolean prepare(DataPlaneNode node, ConfigDelta delta) {
                throw new RuntimeException("network error");
            }
            @Override
            public boolean commit(DataPlaneNode node, ConfigDelta delta) { return true; }
            @Override
            public boolean abort(DataPlaneNode node, ConfigDelta delta) { return true; }
        }, Duration.ofSeconds(5));

        AtomicRollout.RolloutResult result = rollout.execute(createDelta(), List.of(createNode("node-1")));
        assertEquals(AtomicRollout.Phase.ABORTED, result.phase());
        assertEquals(AtomicRollout.AckStatus.PREPARE_NACK, result.nodeStatus().get("node-1"));
    }

    @Test
    void commitFailureReturnsAborted() {
        // All prepares succeed, but commit fails for node-2
        AtomicRollout rollout = new AtomicRollout(new AtomicRollout.RolloutCallback() {
            @Override
            public boolean prepare(DataPlaneNode node, ConfigDelta delta) { return true; }
            @Override
            public boolean commit(DataPlaneNode node, ConfigDelta delta) {
                return !"node-2".equals(node.nodeId());
            }
            @Override
            public boolean abort(DataPlaneNode node, ConfigDelta delta) { return true; }
        }, Duration.ofSeconds(5));

        List<DataPlaneNode> targets = List.of(createNode("node-1"), createNode("node-2"));
        AtomicRollout.RolloutResult result = rollout.execute(createDelta(), targets);

        assertEquals(AtomicRollout.Phase.ABORTED, result.phase());
        assertFalse(result.isCommitted());
    }

    @Test
    void separateCommitTimeoutUsed() {
        AtomicRollout rollout = new AtomicRollout(
                new AcceptingCallback(), Duration.ofSeconds(5), Duration.ofSeconds(10));

        AtomicRollout.RolloutResult result = rollout.execute(createDelta(), List.of(createNode("node-1")));
        assertEquals(AtomicRollout.Phase.COMMITTED, result.phase());
    }

    @Test
    void commitRetryOnFailure() {
        AtomicInteger commitAttempts = new AtomicInteger(0);
        AtomicRollout rollout = new AtomicRollout(new AtomicRollout.RolloutCallback() {
            @Override
            public boolean prepare(DataPlaneNode node, ConfigDelta delta) { return true; }
            @Override
            public boolean commit(DataPlaneNode node, ConfigDelta delta) {
                // First attempt fails, second succeeds
                return commitAttempts.incrementAndGet() > 1;
            }
            @Override
            public boolean abort(DataPlaneNode node, ConfigDelta delta) { return true; }
        }, Duration.ofSeconds(5));

        AtomicRollout.RolloutResult result = rollout.execute(createDelta(), List.of(createNode("node-1")));
        assertEquals(AtomicRollout.Phase.COMMITTED, result.phase());
        assertEquals(2, commitAttempts.get()); // 1 initial + 1 retry
    }

    @Test
    void abortWaitsForCompletion() {
        AtomicBoolean abortCompleted = new AtomicBoolean(false);
        AtomicRollout rollout = new AtomicRollout(new AtomicRollout.RolloutCallback() {
            @Override
            public boolean prepare(DataPlaneNode node, ConfigDelta delta) {
                return !"node-2".equals(node.nodeId());
            }
            @Override
            public boolean commit(DataPlaneNode node, ConfigDelta delta) { return true; }
            @Override
            public boolean abort(DataPlaneNode node, ConfigDelta delta) {
                try { Thread.sleep(50); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                abortCompleted.set(true);
                return true;
            }
        }, Duration.ofSeconds(5));

        List<DataPlaneNode> targets = List.of(createNode("node-1"), createNode("node-2"));
        rollout.execute(createDelta(), targets);

        // After execute returns, abort should have completed (not fire-and-forget)
        assertTrue(abortCompleted.get(), "Abort should have completed before execute returned");
    }

    // ---- Helpers ----

    private static ConfigDelta createDelta() {
        ConfigMutation mutation = new ConfigMutation.Upsert(new ConfigResource(
                new ConfigResourceId("cluster", "global", "test-cluster"),
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1L, Instant.now(), Instant.now(), "test",
                Map.of(),
                new ClusterSpec("test-cluster", "round-robin", "default-hc", 10000, 30)
        ));
        return new ConfigDelta(0, 1, List.of(mutation));
    }

    private static DataPlaneNode createNode(String nodeId) {
        NodeIdentity identity = NodeIdentity.newBuilder()
                .setNodeId(nodeId)
                .setClusterId("test-cluster")
                .setEnvironment("test")
                .setAddress("127.0.0.1")
                .setBuildVersion("1.0.0")
                .build();
        return new DataPlaneNode(identity, "session-" + nodeId);
    }

    private static class AcceptingCallback implements AtomicRollout.RolloutCallback {
        @Override public boolean prepare(DataPlaneNode node, ConfigDelta delta) { return true; }
        @Override public boolean commit(DataPlaneNode node, ConfigDelta delta) { return true; }
        @Override public boolean abort(DataPlaneNode node, ConfigDelta delta) { return true; }
    }

    private record SelectiveRejectCallback(Set<String> rejectNodes) implements AtomicRollout.RolloutCallback {
        @Override public boolean prepare(DataPlaneNode node, ConfigDelta delta) {
            return !rejectNodes.contains(node.nodeId());
        }
        @Override public boolean commit(DataPlaneNode node, ConfigDelta delta) { return true; }
        @Override public boolean abort(DataPlaneNode node, ConfigDelta delta) { return true; }
    }
}
