package com.shieldblaze.expressgateway.controlplane.distribution;

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class DirectFanOutTest {

    record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() { }
    }

    private DataPlaneNode makeNode(String nodeId) {
        NodeIdentity identity = NodeIdentity.newBuilder()
                .setNodeId(nodeId)
                .setClusterId("cluster-1")
                .setEnvironment("test")
                .setAddress("10.0.0." + nodeId.hashCode() % 256)
                .setBuildVersion("1.0.0")
                .build();
        return new DataPlaneNode(identity, "token-" + nodeId);
    }

    private ConfigDelta sampleDelta() {
        ConfigResourceId id = new ConfigResourceId("cluster", "global", "prod");
        ConfigResource resource = new ConfigResource(
                id,
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1,
                Instant.now(),
                Instant.now(),
                "admin",
                Map.of(),
                new TestSpec("test")
        );
        ConfigMutation mutation = new ConfigMutation.Upsert(resource);
        return new ConfigDelta(0, 1, List.of(mutation));
    }

    @Test
    void pushesToAllTargets() {
        List<String> pushedNodeIds = new CopyOnWriteArrayList<>();
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> {
            pushedNodeIds.add(node.nodeId());
            return true;
        });

        List<DataPlaneNode> targets = List.of(
                makeNode("node-1"),
                makeNode("node-2"),
                makeNode("node-3")
        );

        fanOut.distribute(sampleDelta(), targets);

        assertEquals(3, pushedNodeIds.size());
        assertTrue(pushedNodeIds.contains("node-1"));
        assertTrue(pushedNodeIds.contains("node-2"));
        assertTrue(pushedNodeIds.contains("node-3"));
    }

    @Test
    void individualNodeFailureDoesNotAbortOtherPushes() {
        List<String> successfulPushes = new CopyOnWriteArrayList<>();
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> {
            if ("node-2".equals(node.nodeId())) {
                throw new RuntimeException("Simulated transport failure for node-2");
            }
            successfulPushes.add(node.nodeId());
            return true;
        });

        List<DataPlaneNode> targets = List.of(
                makeNode("node-1"),
                makeNode("node-2"),
                makeNode("node-3")
        );

        fanOut.distribute(sampleDelta(), targets);

        assertEquals(2, successfulPushes.size(), "Failure on node-2 should not prevent pushes to node-1 and node-3");
        assertTrue(successfulPushes.contains("node-1"));
        assertTrue(successfulPushes.contains("node-3"));
    }

    @Test
    void nackIsLoggedButDoesNotAbortOtherPushes() {
        List<String> allPushedNodeIds = new CopyOnWriteArrayList<>();
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> {
            allPushedNodeIds.add(node.nodeId());
            // node-2 NACKs (returns false)
            return !"node-2".equals(node.nodeId());
        });

        List<DataPlaneNode> targets = List.of(
                makeNode("node-1"),
                makeNode("node-2"),
                makeNode("node-3")
        );

        fanOut.distribute(sampleDelta(), targets);

        assertEquals(3, allPushedNodeIds.size(), "All nodes should receive a push attempt even if one NACKs");
        assertTrue(allPushedNodeIds.contains("node-1"));
        assertTrue(allPushedNodeIds.contains("node-2"));
        assertTrue(allPushedNodeIds.contains("node-3"));
    }

    @Test
    void multipleFailuresDoNotPreventRemainingPushes() {
        List<String> successfulPushes = new CopyOnWriteArrayList<>();
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> {
            if ("node-1".equals(node.nodeId()) || "node-3".equals(node.nodeId())) {
                throw new RuntimeException("Simulated failure for " + node.nodeId());
            }
            successfulPushes.add(node.nodeId());
            return true;
        });

        List<DataPlaneNode> targets = List.of(
                makeNode("node-1"),
                makeNode("node-2"),
                makeNode("node-3"),
                makeNode("node-4")
        );

        fanOut.distribute(sampleDelta(), targets);

        assertEquals(2, successfulPushes.size());
        assertTrue(successfulPushes.contains("node-2"));
        assertTrue(successfulPushes.contains("node-4"));
    }

    @Test
    void nullDeltaThrowsNullPointerException() {
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> true);
        assertThrows(NullPointerException.class,
                () -> fanOut.distribute(null, List.of(makeNode("node-1"))));
    }

    @Test
    void nullTargetsThrowsNullPointerException() {
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> true);
        assertThrows(NullPointerException.class,
                () -> fanOut.distribute(sampleDelta(), null));
    }

    @Test
    void nullPushCallbackThrowsNullPointerException() {
        assertThrows(NullPointerException.class,
                () -> new DirectFanOut(null));
    }

    @Test
    void emptyTargetListCompletesWithoutError() {
        List<String> pushedNodeIds = new CopyOnWriteArrayList<>();
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> {
            pushedNodeIds.add(node.nodeId());
            return true;
        });

        fanOut.distribute(sampleDelta(), List.of());

        assertTrue(pushedNodeIds.isEmpty(), "No pushes should occur for empty target list");
    }

    @Test
    void callbackReceivesCorrectDelta() {
        List<ConfigDelta> receivedDeltas = new CopyOnWriteArrayList<>();
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> {
            receivedDeltas.add(delta);
            return true;
        });

        ConfigDelta expectedDelta = sampleDelta();
        fanOut.distribute(expectedDelta, List.of(makeNode("node-1")));

        assertEquals(1, receivedDeltas.size());
        assertEquals(expectedDelta, receivedDeltas.get(0));
    }

    @Test
    void callbackReceivesCorrectNode() {
        List<DataPlaneNode> receivedNodes = new CopyOnWriteArrayList<>();
        DirectFanOut fanOut = new DirectFanOut((node, delta) -> {
            receivedNodes.add(node);
            return true;
        });

        DataPlaneNode target = makeNode("specific-node");
        fanOut.distribute(sampleDelta(), List.of(target));

        assertEquals(1, receivedNodes.size());
        assertEquals("specific-node", receivedNodes.get(0).nodeId());
    }
}
