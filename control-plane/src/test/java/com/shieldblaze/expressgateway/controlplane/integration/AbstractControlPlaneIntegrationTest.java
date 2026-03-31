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
package com.shieldblaze.expressgateway.controlplane.integration;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneServer;
import com.shieldblaze.expressgateway.controlplane.kvstore.StorageConfiguration;
import com.shieldblaze.expressgateway.controlplane.config.ChangeJournal;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigTransaction;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigAck;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigDistributionServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigRequest;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigResponse;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigSubscription;
import com.shieldblaze.expressgateway.controlplane.v1.DeregisterRequest;
import com.shieldblaze.expressgateway.controlplane.v1.DeregisterResponse;
import com.shieldblaze.expressgateway.controlplane.v1.DeregistrationReason;
import com.shieldblaze.expressgateway.controlplane.v1.HeartbeatResponse;
import com.shieldblaze.expressgateway.controlplane.v1.NodeHeartbeat;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import com.shieldblaze.expressgateway.controlplane.v1.NodeRegistrationServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.RegisterRequest;
import com.shieldblaze.expressgateway.controlplane.v1.RegisterResponse;
import io.grpc.ManagedChannel;
import io.grpc.Metadata;
import io.grpc.netty.NettyChannelBuilder;
import io.grpc.stub.MetadataUtils;
import io.grpc.stub.StreamObserver;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Tag;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestInstance;
import org.junit.jupiter.api.Timeout;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Abstract end-to-end integration test for the full control plane config distribution
 * pipeline. Each concrete subclass provides a real KV store backend via Testcontainers.
 *
 * <p>Tests exercise the complete path: gRPC registration, heartbeat, config subscription,
 * mutation batching, journal persistence, delta sync, ACK/NACK, compaction, and fan-out --
 * all against a real KV store backend.</p>
 *
 * <p>Uses {@link TestInstance.Lifecycle#PER_CLASS} so that the ControlPlaneServer is shared
 * across all test methods in a single class. Each test uses unique node IDs to avoid
 * cross-test interference.</p>
 */
@Tag("integration")
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
public abstract class AbstractControlPlaneIntegrationTest {

    /**
     * Subclasses provide a connected KVStore backed by a Testcontainer.
     */
    protected abstract KVStore createKVStore() throws Exception;

    /**
     * Returns the KvStoreType enum value matching the backend under test.
     */
    protected abstract ControlPlaneConfiguration.KvStoreType kvStoreType();

    protected ControlPlaneServer cpServer;
    protected int actualGrpcPort;

    @TempDir
    Path tempDir;

    @BeforeAll
    void startControlPlane() throws Exception {
        KVStore kvStore = createKVStore();

        // StorageConfiguration requires non-empty endpoints for validation even though
        // we pass a pre-built KVStore. Provide a dummy value to satisfy validation.
        StorageConfiguration storageConfig = new StorageConfiguration()
                .endpoints(List.of("dummy://localhost:0"));

        ControlPlaneConfiguration config = new ControlPlaneConfiguration()
                .grpcPort(0)                    // ephemeral port
                .heartbeatIntervalMs(5000)
                .heartbeatMissThreshold(3)
                .heartbeatDisconnectThreshold(6)
                .heartbeatScanIntervalMs(2000)
                .writeBatchWindowMs(100)         // fast batching for tests
                .maxJournalLag(100)
                .kvStoreType(kvStoreType())
                .maxRequestsPerSecondPerNode(1000) // high limit for tests
                .storage(storageConfig)
                .validate();

        cpServer = new ControlPlaneServer(config, kvStore);
        cpServer.start();
        actualGrpcPort = cpServer.grpcPort();
        assertTrue(actualGrpcPort > 0, "Expected a valid ephemeral port, got: " + actualGrpcPort);
    }

    @AfterAll
    void stopControlPlane() throws IOException {
        if (cpServer != null) {
            cpServer.close();
        }
    }

    // ---- Helpers ----

    /**
     * Create a plaintext gRPC channel to the test CP server.
     */
    private ManagedChannel createChannel() {
        return NettyChannelBuilder.forAddress("127.0.0.1", actualGrpcPort)
                .usePlaintext()
                .build();
    }

    /**
     * Build metadata with session token and node ID for authenticated stubs.
     */
    private Metadata authMetadata(String nodeId, String sessionToken) {
        Metadata metadata = new Metadata();
        metadata.put(Metadata.Key.of("x-session-token", Metadata.ASCII_STRING_MARSHALLER), sessionToken);
        metadata.put(Metadata.Key.of("x-node-id", Metadata.ASCII_STRING_MARSHALLER), nodeId);
        return metadata;
    }

    /**
     * Register a node via the unary Register RPC and return the response.
     */
    private RegisterResponse registerNode(ManagedChannel channel, String nodeId) {
        NodeRegistrationServiceGrpc.NodeRegistrationServiceBlockingStub stub =
                NodeRegistrationServiceGrpc.newBlockingStub(channel);
        return stub.register(RegisterRequest.newBuilder()
                .setIdentity(NodeIdentity.newBuilder()
                        .setNodeId(nodeId)
                        .setClusterId("test-cluster")
                        .setEnvironment("test")
                        .setAddress("127.0.0.1")
                        .setBuildVersion("1.0.0")
                        .build())
                .setAuthToken("default")
                .addSubscribedResourceTypes("cluster")
                .build());
    }

    /**
     * Create a test ConfigResource for a cluster.
     */
    private ConfigResource createClusterResource(String name, String lbStrategy) {
        ConfigKind kind = ConfigKind.CLUSTER;
        ConfigScope scope = new ConfigScope.Global();
        ConfigResourceId id = new ConfigResourceId(kind.name(), scope.qualifier(), name);
        ClusterSpec spec = new ClusterSpec(name, lbStrategy, "default-hc", 1000, 30);
        Instant now = Instant.now();
        return new ConfigResource(id, kind, scope, 1, now, now, "test-admin", Map.of(), spec);
    }

    /**
     * Submit a config transaction with a single cluster upsert via the distributor.
     */
    private void submitClusterMutation(String clusterName, String lbStrategy) {
        ConfigResource resource = createClusterResource(clusterName, lbStrategy);
        ConfigTransaction tx = ConfigTransaction.builder("test@example.com")
                .description("test mutation: " + clusterName)
                .upsert(resource)
                .build();
        cpServer.configDistributor().submit(tx);
    }

    /**
     * Poll a condition with a timeout.
     */
    private boolean awaitCondition(long timeoutMs, BooleanCondition condition) throws InterruptedException {
        long deadline = System.currentTimeMillis() + timeoutMs;
        while (System.currentTimeMillis() < deadline) {
            if (condition.check()) {
                return true;
            }
            Thread.sleep(100);
        }
        return condition.check();
    }

    @FunctionalInterface
    private interface BooleanCondition {
        boolean check() throws InterruptedException;
    }

    // ===========================================================================
    // Test 1: Node Registration and Heartbeat
    // ===========================================================================

    @Test
    @Timeout(120)
    void testNodeRegistrationAndHeartbeat() throws Exception {
        ManagedChannel channel = createChannel();
        try {
            // --- Register ---
            RegisterResponse registerResponse = registerNode(channel, "dp-heartbeat-1");
            assertTrue(registerResponse.getAccepted(), "Registration should be accepted");
            assertFalse(registerResponse.getSessionToken().isEmpty(), "Session token must not be empty");
            String sessionToken = registerResponse.getSessionToken();

            // Verify node is in the registry
            Optional<DataPlaneNode> nodeOpt = cpServer.nodeRegistry().get("dp-heartbeat-1");
            assertTrue(nodeOpt.isPresent(), "Node should be in registry after registration");

            // --- Heartbeat bidi stream ---
            CountDownLatch heartbeatAckLatch = new CountDownLatch(1);
            AtomicReference<HeartbeatResponse> heartbeatResponseRef = new AtomicReference<>();

            NodeRegistrationServiceGrpc.NodeRegistrationServiceStub asyncStub =
                    NodeRegistrationServiceGrpc.newStub(channel)
                            .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(
                                    authMetadata("dp-heartbeat-1", sessionToken)));

            StreamObserver<NodeHeartbeat> heartbeatRequestObserver = asyncStub.heartbeat(
                    new StreamObserver<>() {
                        @Override
                        public void onNext(HeartbeatResponse response) {
                            heartbeatResponseRef.set(response);
                            heartbeatAckLatch.countDown();
                        }

                        @Override
                        public void onError(Throwable t) {
                            heartbeatAckLatch.countDown();
                        }

                        @Override
                        public void onCompleted() {
                        }
                    });

            // Send a heartbeat
            heartbeatRequestObserver.onNext(NodeHeartbeat.newBuilder()
                    .setNodeId("dp-heartbeat-1")
                    .setSessionToken(sessionToken)
                    .setActiveConnections(100)
                    .setCpuUtilization(0.5)
                    .setMemoryUtilization(0.3)
                    .build());

            assertTrue(heartbeatAckLatch.await(30, TimeUnit.SECONDS),
                    "Heartbeat ACK should arrive within 30 seconds");

            HeartbeatResponse hbResponse = heartbeatResponseRef.get();
            assertNotNull(hbResponse, "Heartbeat response must not be null");
            assertTrue(hbResponse.getDirectivesCount() > 0, "Should have at least one directive");
            assertTrue(hbResponse.getDirectives(0).getAck(), "First directive should be an ACK");

            // Close heartbeat stream
            heartbeatRequestObserver.onCompleted();

            // --- Deregister ---
            NodeRegistrationServiceGrpc.NodeRegistrationServiceBlockingStub blockingStub =
                    NodeRegistrationServiceGrpc.newBlockingStub(channel)
                            .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(
                                    authMetadata("dp-heartbeat-1", sessionToken)));

            DeregisterResponse deregisterResponse = blockingStub.deregister(DeregisterRequest.newBuilder()
                    .setNodeId("dp-heartbeat-1")
                    .setSessionToken(sessionToken)
                    .setReason(DeregistrationReason.SHUTDOWN)
                    .build());

            assertTrue(deregisterResponse.getAcknowledged(), "Deregistration should be acknowledged");

            // Verify node removed
            Optional<DataPlaneNode> removedNode = cpServer.nodeRegistry().get("dp-heartbeat-1");
            assertTrue(removedNode.isEmpty(), "Node should be removed from registry after deregistration");

        } finally {
            channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
        }
    }

    // ===========================================================================
    // Test 2: Config Push and ACK
    // ===========================================================================

    @Test
    @Timeout(120)
    void testConfigPushAndAck() throws Exception {
        ManagedChannel channel = createChannel();
        try {
            // Register
            RegisterResponse registerResponse = registerNode(channel, "dp-config-1");
            assertTrue(registerResponse.getAccepted());
            String sessionToken = registerResponse.getSessionToken();

            // Force node to HEALTHY (distributor pushes only to healthy nodes)
            DataPlaneNode node = cpServer.nodeRegistry().get("dp-config-1").orElseThrow();
            node.recordHeartbeat(0, 0, 0.0, 0.0);

            // Open config stream
            CountDownLatch configReceivedLatch = new CountDownLatch(1);
            List<ConfigResponse> receivedConfigs = new CopyOnWriteArrayList<>();

            ConfigDistributionServiceGrpc.ConfigDistributionServiceStub configStub =
                    ConfigDistributionServiceGrpc.newStub(channel)
                            .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(
                                    authMetadata("dp-config-1", sessionToken)));

            StreamObserver<ConfigRequest> configRequestObserver = configStub.streamConfig(
                    new StreamObserver<>() {
                        @Override
                        public void onNext(ConfigResponse response) {
                            receivedConfigs.add(response);
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onError(Throwable t) {
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onCompleted() {
                        }
                    });

            // Subscribe to "cluster"
            configRequestObserver.onNext(ConfigRequest.newBuilder()
                    .setNodeId("dp-config-1")
                    .setSessionToken(sessionToken)
                    .setSubscribe(ConfigSubscription.newBuilder()
                            .setTypeUrl("cluster")
                            .setVersion("0")
                            .build())
                    .build());

            // Allow subscription to register server-side
            Thread.sleep(500);

            // Submit a config mutation on the server side
            submitClusterMutation("test-cluster-push", "round-robin");

            // Wait for the config push
            assertTrue(configReceivedLatch.await(30, TimeUnit.SECONDS),
                    "Should receive config push within 30 seconds");
            assertFalse(receivedConfigs.isEmpty(), "Should have received at least one config response");

            ConfigResponse configResponse = receivedConfigs.get(0);
            assertNotNull(configResponse.getVersion());
            assertFalse(configResponse.getVersion().isEmpty(), "Version must not be empty");

            // Send ACK
            configRequestObserver.onNext(ConfigRequest.newBuilder()
                    .setNodeId("dp-config-1")
                    .setSessionToken(sessionToken)
                    .setAck(ConfigAck.newBuilder()
                            .setTypeUrl("cluster")
                            .setVersion(configResponse.getVersion())
                            .setResponseNonce(configResponse.getNonce())
                            .build())
                    .build());

            // Verify node's appliedConfigVersion was updated
            long expectedVersion = Long.parseLong(configResponse.getVersion());
            boolean versionUpdated = awaitCondition(10_000, () -> {
                Optional<DataPlaneNode> n = cpServer.nodeRegistry().get("dp-config-1");
                return n.isPresent() && n.get().appliedConfigVersion() == expectedVersion;
            });
            assertTrue(versionUpdated,
                    "Node's appliedConfigVersion should be " + expectedVersion +
                            ", actual: " + cpServer.nodeRegistry().get("dp-config-1")
                            .map(DataPlaneNode::appliedConfigVersion).orElse(-1L));

            configRequestObserver.onCompleted();
        } finally {
            cpServer.nodeRegistry().deregister("dp-config-1");
            channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
        }
    }

    // ===========================================================================
    // Test 3: Delta Sync after Multiple Mutations
    // ===========================================================================

    @Test
    @Timeout(120)
    void testDeltaSyncAfterMultipleMutations() throws Exception {
        ManagedChannel channel = createChannel();
        try {
            // Register
            RegisterResponse registerResponse = registerNode(channel, "dp-delta-1");
            assertTrue(registerResponse.getAccepted());
            String sessionToken = registerResponse.getSessionToken();

            // Force HEALTHY
            DataPlaneNode node = cpServer.nodeRegistry().get("dp-delta-1").orElseThrow();
            node.recordHeartbeat(0, 0, 0.0, 0.0);

            // Open config stream
            CountDownLatch configReceivedLatch = new CountDownLatch(1);
            List<ConfigResponse> receivedConfigs = new CopyOnWriteArrayList<>();

            ConfigDistributionServiceGrpc.ConfigDistributionServiceStub configStub =
                    ConfigDistributionServiceGrpc.newStub(channel)
                            .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(
                                    authMetadata("dp-delta-1", sessionToken)));

            StreamObserver<ConfigRequest> configRequestObserver = configStub.streamConfig(
                    new StreamObserver<>() {
                        @Override
                        public void onNext(ConfigResponse response) {
                            receivedConfigs.add(response);
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onError(Throwable t) {
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onCompleted() {
                        }
                    });

            // Subscribe
            configRequestObserver.onNext(ConfigRequest.newBuilder()
                    .setNodeId("dp-delta-1")
                    .setSessionToken(sessionToken)
                    .setSubscribe(ConfigSubscription.newBuilder()
                            .setTypeUrl("cluster")
                            .setVersion("0")
                            .build())
                    .build());

            Thread.sleep(500);

            // Submit 3 mutations: create cluster-1, create cluster-2, update cluster-1
            submitClusterMutation("delta-cluster-1", "round-robin");
            submitClusterMutation("delta-cluster-2", "least-connection");
            submitClusterMutation("delta-cluster-1", "random");  // update

            // Wait for config push(es)
            assertTrue(configReceivedLatch.await(30, TimeUnit.SECONDS),
                    "Should receive config push within 30 seconds");
            assertFalse(receivedConfigs.isEmpty(), "Should have received config responses");

            // Wait for all resources to arrive (may come in multiple pushes)
            boolean gotAllConfigs = awaitCondition(15_000, () -> {
                int totalResources = 0;
                for (ConfigResponse r : receivedConfigs) {
                    totalResources += r.getResourcesCount();
                }
                return totalResources >= 2;
            });
            assertTrue(gotAllConfigs,
                    "Should receive at least 2 resources (delta-cluster-1 latest + delta-cluster-2)");

            // Verify both clusters are present
            List<String> allResourceNames = new ArrayList<>();
            for (ConfigResponse r : receivedConfigs) {
                for (int i = 0; i < r.getResourcesCount(); i++) {
                    allResourceNames.add(r.getResources(i).getName());
                }
            }
            assertTrue(allResourceNames.contains("delta-cluster-1"), "Should contain delta-cluster-1");
            assertTrue(allResourceNames.contains("delta-cluster-2"), "Should contain delta-cluster-2");

            configRequestObserver.onCompleted();
        } finally {
            cpServer.nodeRegistry().deregister("dp-delta-1");
            channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
        }
    }

    // ===========================================================================
    // Test 4: Full Snapshot on Revision Zero
    // ===========================================================================

    @Test
    @Timeout(120)
    void testFullSnapshotOnRevisionZero() throws Exception {
        // Submit mutations BEFORE any node connects
        submitClusterMutation("pre-existing-cluster", "round-robin");

        // Wait for journaling
        boolean journaled = awaitCondition(10_000, () ->
                cpServer.changeJournal().currentRevision() > 0);
        assertTrue(journaled, "Journal should have at least one entry");

        ManagedChannel channel = createChannel();
        try {
            // Register a brand-new node
            RegisterResponse registerResponse = registerNode(channel, "dp-snapshot-1");
            assertTrue(registerResponse.getAccepted());
            String sessionToken = registerResponse.getSessionToken();

            // Force HEALTHY
            DataPlaneNode node = cpServer.nodeRegistry().get("dp-snapshot-1").orElseThrow();
            node.recordHeartbeat(0, 0, 0.0, 0.0);

            // Open config stream
            CountDownLatch configReceivedLatch = new CountDownLatch(1);
            List<ConfigResponse> receivedConfigs = new CopyOnWriteArrayList<>();

            ConfigDistributionServiceGrpc.ConfigDistributionServiceStub configStub =
                    ConfigDistributionServiceGrpc.newStub(channel)
                            .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(
                                    authMetadata("dp-snapshot-1", sessionToken)));

            StreamObserver<ConfigRequest> configRequestObserver = configStub.streamConfig(
                    new StreamObserver<>() {
                        @Override
                        public void onNext(ConfigResponse response) {
                            receivedConfigs.add(response);
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onError(Throwable t) {
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onCompleted() {
                        }
                    });

            // Subscribe with version "0" (never received config)
            configRequestObserver.onNext(ConfigRequest.newBuilder()
                    .setNodeId("dp-snapshot-1")
                    .setSessionToken(sessionToken)
                    .setSubscribe(ConfigSubscription.newBuilder()
                            .setTypeUrl("cluster")
                            .setVersion("0")
                            .build())
                    .build());

            assertTrue(configReceivedLatch.await(30, TimeUnit.SECONDS),
                    "Should receive initial config within 30 seconds");
            assertFalse(receivedConfigs.isEmpty(), "Should have received at least one config response");

            ConfigResponse firstResponse = receivedConfigs.get(0);
            // Node at version 0 should receive all resources (delta or full snapshot)
            assertTrue(firstResponse.getResourcesCount() > 0 || firstResponse.getIsFullSnapshot(),
                    "Should receive resources or full snapshot flag for a fresh node");

            configRequestObserver.onCompleted();
        } finally {
            cpServer.nodeRegistry().deregister("dp-snapshot-1");
            channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
        }
    }

    // ===========================================================================
    // Test 5: Journal Compaction Triggers Snapshot
    // ===========================================================================

    @Test
    @Timeout(120)
    void testJournalCompactionTriggersSnapshot() throws Exception {
        ManagedChannel channel = createChannel();
        try {
            // Register and mark healthy
            RegisterResponse registerResponse = registerNode(channel, "dp-compact-1");
            assertTrue(registerResponse.getAccepted());
            String sessionToken = registerResponse.getSessionToken();

            DataPlaneNode node = cpServer.nodeRegistry().get("dp-compact-1").orElseThrow();
            node.recordHeartbeat(0, 0, 0.0, 0.0);

            ChangeJournal journal = cpServer.changeJournal();
            long startRevision = journal.currentRevision();

            // Submit several mutations
            for (int i = 1; i <= 5; i++) {
                submitClusterMutation("compact-cluster-" + i, "round-robin");
            }

            // Wait for mutations to journal (the batcher may coalesce them into fewer entries)
            boolean allJournaled = awaitCondition(15_000, () ->
                    journal.currentRevision() > startRevision);
            assertTrue(allJournaled,
                    "Mutations should be journaled. Start revision: " + startRevision +
                            ", current: " + journal.currentRevision());

            // Compact the journal up to and including the current revision.
            // After compaction, there are no journal entries left for entriesSince(0).
            long compactUpTo = journal.currentRevision();
            journal.compact(compactUpTo);

            // Open config stream. Node at version 0, all journal entries compacted away.
            // DeltaSyncEngine.computeDelta(0) will find no entries and return null,
            // causing the server to send is_full_snapshot=true.
            CountDownLatch configReceivedLatch = new CountDownLatch(1);
            List<ConfigResponse> receivedConfigs = new CopyOnWriteArrayList<>();

            ConfigDistributionServiceGrpc.ConfigDistributionServiceStub configStub =
                    ConfigDistributionServiceGrpc.newStub(channel)
                            .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(
                                    authMetadata("dp-compact-1", sessionToken)));

            StreamObserver<ConfigRequest> configRequestObserver = configStub.streamConfig(
                    new StreamObserver<>() {
                        @Override
                        public void onNext(ConfigResponse response) {
                            receivedConfigs.add(response);
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onError(Throwable t) {
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onCompleted() {
                        }
                    });

            configRequestObserver.onNext(ConfigRequest.newBuilder()
                    .setNodeId("dp-compact-1")
                    .setSessionToken(sessionToken)
                    .setSubscribe(ConfigSubscription.newBuilder()
                            .setTypeUrl("cluster")
                            .setVersion("0")
                            .build())
                    .build());

            assertTrue(configReceivedLatch.await(30, TimeUnit.SECONDS),
                    "Should receive config after compaction within 30 seconds");
            assertFalse(receivedConfigs.isEmpty(), "Should have received config responses");

            ConfigResponse response = receivedConfigs.get(0);
            // DeltaSyncEngine returns null when journal is compacted past the node's revision,
            // which causes the server to send is_full_snapshot=true.
            assertTrue(response.getIsFullSnapshot(),
                    "Should receive is_full_snapshot=true after journal compaction past node's revision");

            configRequestObserver.onCompleted();
        } finally {
            cpServer.nodeRegistry().deregister("dp-compact-1");
            channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
        }
    }

    // ===========================================================================
    // Test 6: Config Receive and ACK Persistence (simulated agent workflow)
    // ===========================================================================

    /**
     * Simulates the ControlPlaneAgent's config-receive-ACK workflow using raw gRPC,
     * and verifies that after receiving config and sending an ACK, the server-side
     * node state reflects the applied version. Also writes a local LKG-equivalent
     * file to verify the data can be persisted as JSON.
     *
     * <p>This avoids a circular Maven dependency by not importing from control-plane-agent.</p>
     */
    @Test
    @Timeout(120)
    void testLKGStorePersistence() throws Exception {
        // Submit config before the agent-like node connects
        submitClusterMutation("lkg-cluster", "round-robin");

        boolean journaled = awaitCondition(10_000, () ->
                cpServer.changeJournal().currentRevision() > 0);
        assertTrue(journaled, "Config should be journaled");

        ManagedChannel channel = createChannel();
        try {
            // Register
            RegisterResponse registerResponse = registerNode(channel, "dp-lkg-1");
            assertTrue(registerResponse.getAccepted());
            String sessionToken = registerResponse.getSessionToken();

            // Force HEALTHY
            DataPlaneNode node = cpServer.nodeRegistry().get("dp-lkg-1").orElseThrow();
            node.recordHeartbeat(0, 0, 0.0, 0.0);

            // Open config stream (simulating what the agent does)
            CountDownLatch configReceivedLatch = new CountDownLatch(1);
            List<ConfigResponse> receivedConfigs = new CopyOnWriteArrayList<>();

            ConfigDistributionServiceGrpc.ConfigDistributionServiceStub configStub =
                    ConfigDistributionServiceGrpc.newStub(channel)
                            .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(
                                    authMetadata("dp-lkg-1", sessionToken)));

            StreamObserver<ConfigRequest> configRequestObserver = configStub.streamConfig(
                    new StreamObserver<>() {
                        @Override
                        public void onNext(ConfigResponse response) {
                            receivedConfigs.add(response);
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onError(Throwable t) {
                            configReceivedLatch.countDown();
                        }

                        @Override
                        public void onCompleted() {
                        }
                    });

            // Subscribe
            configRequestObserver.onNext(ConfigRequest.newBuilder()
                    .setNodeId("dp-lkg-1")
                    .setSessionToken(sessionToken)
                    .setSubscribe(ConfigSubscription.newBuilder()
                            .setTypeUrl("cluster")
                            .setVersion("0")
                            .build())
                    .build());

            assertTrue(configReceivedLatch.await(30, TimeUnit.SECONDS),
                    "Should receive config within 30 seconds");
            assertFalse(receivedConfigs.isEmpty(), "Should have received config");

            ConfigResponse configResponse = receivedConfigs.get(0);
            long version = Long.parseLong(configResponse.getVersion());

            // Send ACK (what the agent does before writing LKG)
            configRequestObserver.onNext(ConfigRequest.newBuilder()
                    .setNodeId("dp-lkg-1")
                    .setSessionToken(sessionToken)
                    .setAck(ConfigAck.newBuilder()
                            .setTypeUrl("cluster")
                            .setVersion(configResponse.getVersion())
                            .setResponseNonce(configResponse.getNonce())
                            .build())
                    .build());

            // Verify server-side version update
            boolean versionUpdated = awaitCondition(10_000, () -> {
                Optional<DataPlaneNode> n = cpServer.nodeRegistry().get("dp-lkg-1");
                return n.isPresent() && n.get().appliedConfigVersion() == version;
            });
            assertTrue(versionUpdated, "Node's appliedConfigVersion should match ACK'd version");

            // Simulate LKG persistence: write the received config to a local file as JSON
            // (this is what the agent's LKGStore.save() does)
            Path lkgPath = tempDir.resolve("lkg-test.json");
            ObjectMapper mapper = new ObjectMapper();
            mapper.findAndRegisterModules();

            // Build a simple snapshot from the response
            List<Map<String, Object>> resources = new ArrayList<>();
            for (int i = 0; i < configResponse.getResourcesCount(); i++) {
                var resource = configResponse.getResources(i);
                resources.add(Map.of(
                        "name", resource.getName(),
                        "typeUrl", resource.getTypeUrl(),
                        "version", resource.getVersion()
                ));
            }
            Map<String, Object> snapshot = Map.of("revision", version, "resources", resources);
            Files.writeString(lkgPath, mapper.writeValueAsString(snapshot));

            // Verify the LKG file exists and contains the correct revision
            assertTrue(Files.exists(lkgPath), "LKG file should exist");
            var lkgNode = mapper.readTree(lkgPath.toFile());
            long storedRevision = lkgNode.get("revision").asLong(-1);
            assertTrue(storedRevision > 0, "Stored LKG revision should be > 0, got: " + storedRevision);

            configRequestObserver.onCompleted();
        } finally {
            cpServer.nodeRegistry().deregister("dp-lkg-1");
            channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
        }
    }

    // ===========================================================================
    // Test 7: Multiple Nodes Fan-Out
    // ===========================================================================

    @Test
    @Timeout(120)
    void testMultipleNodesFanOut() throws Exception {
        int nodeCount = 3;
        List<ManagedChannel> channels = new ArrayList<>();
        List<StreamObserver<ConfigRequest>> requestObservers = new ArrayList<>();
        List<CountDownLatch> latches = new ArrayList<>();
        List<CopyOnWriteArrayList<ConfigResponse>> allReceivedConfigs = new ArrayList<>();

        try {
            // Register and subscribe 3 nodes
            for (int i = 0; i < nodeCount; i++) {
                String nodeId = "dp-fanout-" + i;
                ManagedChannel channel = createChannel();
                channels.add(channel);

                RegisterResponse registerResponse = registerNode(channel, nodeId);
                assertTrue(registerResponse.getAccepted(),
                        "Registration should be accepted for " + nodeId);
                String sessionToken = registerResponse.getSessionToken();

                // Force HEALTHY
                DataPlaneNode node = cpServer.nodeRegistry().get(nodeId).orElseThrow();
                node.recordHeartbeat(0, 0, 0.0, 0.0);

                // Open config stream
                CountDownLatch latch = new CountDownLatch(1);
                latches.add(latch);
                CopyOnWriteArrayList<ConfigResponse> receivedConfigs = new CopyOnWriteArrayList<>();
                allReceivedConfigs.add(receivedConfigs);

                ConfigDistributionServiceGrpc.ConfigDistributionServiceStub configStub =
                        ConfigDistributionServiceGrpc.newStub(channel)
                                .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(
                                        authMetadata(nodeId, sessionToken)));

                StreamObserver<ConfigRequest> requestObserver = configStub.streamConfig(
                        new StreamObserver<>() {
                            @Override
                            public void onNext(ConfigResponse response) {
                                receivedConfigs.add(response);
                                latch.countDown();
                            }

                            @Override
                            public void onError(Throwable t) {
                                latch.countDown();
                            }

                            @Override
                            public void onCompleted() {
                            }
                        });
                requestObservers.add(requestObserver);

                // Subscribe
                requestObserver.onNext(ConfigRequest.newBuilder()
                        .setNodeId(nodeId)
                        .setSessionToken(sessionToken)
                        .setSubscribe(ConfigSubscription.newBuilder()
                                .setTypeUrl("cluster")
                                .setVersion("0")
                                .build())
                        .build());
            }

            // Allow subscriptions to register
            Thread.sleep(500);

            // Submit a config mutation
            submitClusterMutation("fanout-cluster", "round-robin");

            // All 3 nodes must receive the push
            for (int i = 0; i < nodeCount; i++) {
                assertTrue(latches.get(i).await(30, TimeUnit.SECONDS),
                        "Node dp-fanout-" + i + " should receive config push within 30 seconds");
                assertFalse(allReceivedConfigs.get(i).isEmpty(),
                        "Node dp-fanout-" + i + " should have received config responses");
            }

            // Close streams
            for (StreamObserver<ConfigRequest> observer : requestObservers) {
                observer.onCompleted();
            }

        } finally {
            for (int i = 0; i < nodeCount; i++) {
                cpServer.nodeRegistry().deregister("dp-fanout-" + i);
            }
            for (ManagedChannel channel : channels) {
                channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
            }
        }
    }
}
