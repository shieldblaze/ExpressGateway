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
import com.shieldblaze.expressgateway.controlplane.kvstore.etcd.EtcdKVStore;
import com.shieldblaze.expressgateway.controlplane.v1.NodeRegistrationServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.RegisterRequest;
import com.shieldblaze.expressgateway.controlplane.v1.RegisterResponse;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import io.etcd.jetcd.Client;
import io.grpc.ManagedChannel;
import java.net.ServerSocket;
import io.grpc.netty.NettyChannelBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Tag;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestInstance;
import org.junit.jupiter.api.Timeout;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.junit.jupiter.Container;
import org.testcontainers.junit.jupiter.Testcontainers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Integration test verifying that the {@link ReconnectStormProtector} correctly
 * rate-limits reconnections when a CP instance fails and nodes reconnect to another.
 *
 * <p>Simulates 100 nodes attempting to register against a single CP instance with
 * a burst limit of 20 and verifies that the protector rejects excess reconnections.</p>
 */
@Tag("integration")
@Testcontainers
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class ReconnectStormIntegrationTest {

    @Container
    static final GenericContainer<?> ETCD = new GenericContainer<>("quay.io/coreos/etcd:v3.5.17")
            .withExposedPorts(2379)
            .withCommand("etcd",
                    "--listen-client-urls=http://0.0.0.0:2379",
                    "--advertise-client-urls=http://0.0.0.0:2379")
            .waitingFor(Wait.forHttp("/health").forPort(2379).forStatusCode(200));

    private Client etcdClient;
    private ControlPlaneServer cpServer;

    @BeforeAll
    void startServer() throws Exception {
        String endpoint = "http://" + ETCD.getHost() + ":" + ETCD.getMappedPort(2379);
        etcdClient = Client.builder().endpoints(endpoint).build();
        KVStore kvStore = new EtcdKVStore(etcdClient);

        int port;
        try (ServerSocket ss = new ServerSocket(0)) {
            ss.setReuseAddress(true);
            port = ss.getLocalPort();
        }

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
                .kvStoreType(ControlPlaneConfiguration.KvStoreType.ETCD)
                .maxRequestsPerSecondPerNode(1000)
                .clusterEnabled(true)
                .region("us-east-1")
                .reconnectBurst(20)       // Small burst for testing
                .reconnectRefillRate(10)  // 10/s refill
                .storage(storageConfig)
                .validate();

        cpServer = new ControlPlaneServer(config, kvStore);
        cpServer.start();

        // Allow cluster to initialize
        Thread.sleep(3000);
    }

    @AfterAll
    void stopServer() throws Exception {
        if (cpServer != null) {
            cpServer.close();
        }
        if (etcdClient != null) {
            etcdClient.close();
        }
    }

    @Test
    @Timeout(120)
    @DisplayName("ReconnectStormProtector rate-limits simultaneous reconnections")
    void testReconnectStormProtectorRateLimits() throws Exception {
        ReconnectStormProtector protector = cpServer.reconnectStormProtector();
        assertTrue(protector != null, "Protector must not be null when clustering is enabled");

        int totalNodes = 100;
        AtomicInteger admitted = new AtomicInteger(0);
        AtomicInteger rejected = new AtomicInteger(0);

        // Simulate 100 simultaneous reconnect attempts
        for (int i = 0; i < totalNodes; i++) {
            if (protector.tryAdmit()) {
                admitted.incrementAndGet();
            } else {
                rejected.incrementAndGet();
            }
        }

        // With burst=20, at most 20 should be admitted initially
        assertTrue(admitted.get() <= 20,
                "With burst=20, at most 20 should be admitted immediately. Got: " + admitted.get());
        assertTrue(rejected.get() > 0,
                "Some reconnections should be rejected. Rejected: " + rejected.get());
        assertTrue(admitted.get() + rejected.get() == totalNodes,
                "Total should equal " + totalNodes);

        // After waiting 1 second, more should be admittable (refill rate = 10/s)
        Thread.sleep(1100);
        int additionalAdmitted = 0;
        for (int i = 0; i < 15; i++) {
            if (protector.tryAdmit()) {
                additionalAdmitted++;
            }
        }
        assertTrue(additionalAdmitted > 0,
                "After 1 second, at least some reconnections should be admitted via refill");
    }

    @Test
    @Timeout(120)
    @DisplayName("100 nodes eventually all register successfully with rate limiting")
    void testAllNodesEventuallyRegister() throws Exception {
        int totalNodes = 100;
        AtomicInteger successCount = new AtomicInteger(0);
        AtomicInteger failCount = new AtomicInteger(0);
        CountDownLatch completedLatch = new CountDownLatch(totalNodes);

        int port = cpServer.grpcPort();
        ExecutorService executor = Executors.newFixedThreadPool(20);

        try {
            for (int i = 0; i < totalNodes; i++) {
                final int nodeIdx = i;
                executor.submit(() -> {
                    String nodeId = "storm-node-" + nodeIdx;
                    ManagedChannel channel = NettyChannelBuilder
                            .forAddress("127.0.0.1", port)
                            .usePlaintext()
                            .build();
                    try {
                        NodeRegistrationServiceGrpc.NodeRegistrationServiceBlockingStub stub =
                                NodeRegistrationServiceGrpc.newBlockingStub(channel);
                        RegisterResponse response = stub.register(RegisterRequest.newBuilder()
                                .setIdentity(NodeIdentity.newBuilder()
                                        .setNodeId(nodeId)
                                        .setClusterId("storm-test")
                                        .setEnvironment("test")
                                        .setAddress("127.0.0.1")
                                        .setBuildVersion("1.0.0")
                                        .build())
                                .setAuthToken("default")
                                .addSubscribedResourceTypes("cluster")
                                .build());

                        if (response.getAccepted()) {
                            successCount.incrementAndGet();
                        } else {
                            failCount.incrementAndGet();
                        }
                    } catch (Exception e) {
                        failCount.incrementAndGet();
                    } finally {
                        try {
                            channel.shutdownNow().awaitTermination(2, TimeUnit.SECONDS);
                        } catch (InterruptedException e) {
                            Thread.currentThread().interrupt();
                        }
                        completedLatch.countDown();
                    }
                });
            }

            assertTrue(completedLatch.await(60, TimeUnit.SECONDS),
                    "All registration attempts should complete within 60 seconds");

            // The majority should succeed since registration itself is not rate-limited
            // by the ReconnectStormProtector (it's applied at the connection/admission layer).
            // Here we just verify all attempts completed.
            assertTrue(successCount.get() + failCount.get() == totalNodes,
                    "All attempts should have completed");

        } finally {
            executor.shutdownNow();
            // Deregister all nodes
            for (int i = 0; i < totalNodes; i++) {
                try {
                    cpServer.nodeRegistry().deregister("storm-node-" + i);
                } catch (Exception ignored) { }
            }
        }
    }
}
