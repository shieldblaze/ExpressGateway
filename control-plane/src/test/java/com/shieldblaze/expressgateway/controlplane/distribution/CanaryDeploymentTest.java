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
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class CanaryDeploymentTest {

    @Test
    void successfulCanaryPromotion() {
        AtomicInteger pushCount = new AtomicInteger();
        CanaryDeployment deployment = new CanaryDeployment(
                (node, delta) -> { pushCount.incrementAndGet(); return true; },
                (nodes, criteria) -> true, // always healthy
                0.5, 1,
                new CanaryDeployment.CanaryCriteria(0.01, 100, Duration.ofMillis(10))
        );

        List<DataPlaneNode> targets = List.of(
                createNode("node-1"), createNode("node-2"),
                createNode("node-3"), createNode("node-4")
        );

        CanaryDeployment.DeploymentResult result = deployment.execute(createDelta(), targets);

        assertEquals(CanaryDeployment.DeploymentState.PROMOTED, result.state());
        assertEquals(4, pushCount.get()); // All 4 nodes should have been pushed to
        assertTrue(result.canaryNodes().size() >= 2); // At least 50%
    }

    @Test
    void canaryFailureCausesRollbackToCanaryNodes() {
        // Track which nodes receive rollback
        CopyOnWriteArrayList<String> rolledBackNodes = new CopyOnWriteArrayList<>();

        CanaryDeployment deployment = new CanaryDeployment(
                (node, delta) -> true, // push succeeds
                (nodes, criteria) -> false, // canary health check fails
                (node, delta) -> { rolledBackNodes.add(node.nodeId()); return true; },
                1.0, 1, // 100% canary = all nodes are canary
                new CanaryDeployment.CanaryCriteria(0.01, 100, Duration.ofMillis(10))
        );

        List<DataPlaneNode> targets = List.of(createNode("node-1"), createNode("node-2"));

        CanaryDeployment.DeploymentResult result = deployment.execute(createDelta(), targets);

        assertEquals(CanaryDeployment.DeploymentState.ROLLED_BACK, result.state());
        // All nodes that received canary config should have been rolled back
        assertEquals(2, rolledBackNodes.size());
    }

    @Test
    void canaryPushFailureSendsRollbackToSuccessfulNodes() {
        Set<String> pushedNodes = ConcurrentHashMap.newKeySet();
        CopyOnWriteArrayList<String> rolledBackNodes = new CopyOnWriteArrayList<>();

        // Use 3 nodes: first 2 succeed, node-3 fails. Since nodes are shuffled,
        // we make only "fail-node" reject to ensure at least one node succeeds before failure.
        CanaryDeployment deployment = new CanaryDeployment(
                (node, delta) -> {
                    if ("fail-node".equals(node.nodeId())) {
                        return false; // this node rejects
                    }
                    pushedNodes.add(node.nodeId());
                    return true;
                },
                (nodes, criteria) -> true,
                (node, delta) -> { rolledBackNodes.add(node.nodeId()); return true; },
                1.0, 1, // 100% canary
                new CanaryDeployment.CanaryCriteria(0.01, 100, Duration.ofMillis(10))
        );

        CanaryDeployment.DeploymentResult result = deployment.execute(
                createDelta(), List.of(createNode("ok-1"), createNode("ok-2"), createNode("fail-node")));

        assertEquals(CanaryDeployment.DeploymentState.CANARY_FAILED, result.state());
        // All nodes that were successfully pushed to should be rolled back
        for (String nodeId : pushedNodes) {
            assertTrue(rolledBackNodes.contains(nodeId),
                    "Successfully pushed node " + nodeId + " should have been rolled back");
        }
        // The failing node should NOT be rolled back (it never received the config)
        assertFalse(rolledBackNodes.contains("fail-node"),
                "The failing node should not have been rolled back");
    }

    @Test
    void emptyTargetListPromotesImmediately() {
        CanaryDeployment deployment = new CanaryDeployment(
                (node, delta) -> true,
                (nodes, criteria) -> true,
                0.5, 1,
                new CanaryDeployment.CanaryCriteria(0.01, 100, Duration.ofMillis(10))
        );

        CanaryDeployment.DeploymentResult result = deployment.execute(createDelta(), List.of());
        assertEquals(CanaryDeployment.DeploymentState.PROMOTED, result.state());
    }

    @Test
    void stateTransitionsTracked() {
        CanaryDeployment deployment = new CanaryDeployment(
                (node, delta) -> true,
                (nodes, criteria) -> true,
                0.5, 1,
                new CanaryDeployment.CanaryCriteria(0.01, 100, Duration.ofMillis(10))
        );

        assertEquals(CanaryDeployment.DeploymentState.NOT_STARTED, deployment.currentState());
        deployment.execute(createDelta(), List.of(createNode("node-1")));
        assertEquals(CanaryDeployment.DeploymentState.PROMOTED, deployment.currentState());
    }

    @Test
    void concurrentExecuteRejected() {
        CanaryDeployment deployment = new CanaryDeployment(
                (node, delta) -> true,
                (nodes, criteria) -> true,
                0.5, 1,
                new CanaryDeployment.CanaryCriteria(0.01, 100, Duration.ofMillis(10))
        );

        // First execute
        deployment.execute(createDelta(), List.of(createNode("node-1")));

        // Second execute should be rejected because state is PROMOTED (not NOT_STARTED)
        CanaryDeployment.DeploymentResult secondResult = deployment.execute(
                createDelta(), List.of(createNode("node-2")));

        assertEquals(CanaryDeployment.DeploymentState.PROMOTED, secondResult.state());
        assertTrue(secondResult.message().contains("concurrent execution rejected"));
    }

    @Test
    void minCanaryNodesEnforced() {
        AtomicInteger pushCount = new AtomicInteger();
        CanaryDeployment deployment = new CanaryDeployment(
                (node, delta) -> { pushCount.incrementAndGet(); return true; },
                (nodes, criteria) -> true,
                0.01, // 1% - would be 0 of 2 nodes
                2,    // but minCanaryNodes = 2
                new CanaryDeployment.CanaryCriteria(0.01, 100, Duration.ofMillis(10))
        );

        List<DataPlaneNode> targets = List.of(createNode("node-1"), createNode("node-2"));
        CanaryDeployment.DeploymentResult result = deployment.execute(createDelta(), targets);

        assertEquals(CanaryDeployment.DeploymentState.PROMOTED, result.state());
        // Both nodes should be canary since min=2 and total=2
        assertEquals(2, result.canaryNodes().size());
    }

    @Test
    void promotionFailuresTracked() {
        AtomicInteger pushCount = new AtomicInteger(0);
        CanaryDeployment deployment = new CanaryDeployment(
                (node, delta) -> {
                    // First push (canary) succeeds, remaining node fails
                    return pushCount.incrementAndGet() <= 1;
                },
                (nodes, criteria) -> true,
                0.5, 1,
                new CanaryDeployment.CanaryCriteria(0.01, 100, Duration.ofMillis(10))
        );

        List<DataPlaneNode> targets = List.of(createNode("node-1"), createNode("node-2"));
        CanaryDeployment.DeploymentResult result = deployment.execute(createDelta(), targets);

        // Should still be PROMOTED but with failure info
        assertEquals(CanaryDeployment.DeploymentState.PROMOTED, result.state());
        assertTrue(result.message().contains("failed"));
    }

    // ---- Helpers ----

    private static ConfigDelta createDelta() {
        ConfigMutation mutation = new ConfigMutation.Upsert(new ConfigResource(
                new ConfigResourceId("cluster", "global", "test-cluster"),
                ConfigKind.CLUSTER, new ConfigScope.Global(), 1L,
                Instant.now(), Instant.now(), "test", Map.of(),
                new ClusterSpec("test-cluster", "round-robin", "default-hc", 10000, 30)
        ));
        return new ConfigDelta(0, 1, List.of(mutation));
    }

    private static DataPlaneNode createNode(String nodeId) {
        NodeIdentity identity = NodeIdentity.newBuilder()
                .setNodeId(nodeId).setClusterId("test").setEnvironment("test")
                .setAddress("127.0.0.1").setBuildVersion("1.0.0").build();
        return new DataPlaneNode(identity, "session-" + nodeId);
    }
}
