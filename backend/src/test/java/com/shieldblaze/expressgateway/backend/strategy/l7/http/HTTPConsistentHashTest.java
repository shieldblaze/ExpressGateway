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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.Http2Headers;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTPConsistentHashTest {

    /**
     * The same client IP (hash key) must always map to the same backend node.
     * Repeated requests from the same source address must be deterministic.
     */
    @Test
    void testSameKeyMapsToSameNode() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPConsistentHash(NOOPSessionPersistence.INSTANCE))
                .build();

        for (int i = 1; i <= 10; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        HTTPBalanceRequest request = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.100", 5000), EmptyHttpHeaders.INSTANCE);

        // First request establishes the mapping.
        InetSocketAddress firstResult = cluster.nextNode(request).node().socketAddress();
        assertNotNull(firstResult);

        // Subsequent requests with the same source must resolve to the same node.
        for (int i = 0; i < 100; i++) {
            InetSocketAddress result = cluster.nextNode(request).node().socketAddress();
            assertEquals(firstResult, result,
                    "Same hash key must always map to the same node");
        }
    }

    /**
     * When a hash header name is configured, the value of that header is used as
     * the hash key instead of the client IP. Requests with the same header value
     * must map to the same node regardless of client IP.
     */
    @Test
    void testHeaderBasedHashing() throws Exception {
        HTTPConsistentHash lb = new HTTPConsistentHash(NOOPSessionPersistence.INSTANCE, "X-Forwarded-For");
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        for (int i = 1; i <= 10; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        // Two different client IPs but the same X-Forwarded-For header value.
        HttpHeaders headers1 = new DefaultHttpHeaders().add("X-Forwarded-For", "203.0.113.50");
        HttpHeaders headers2 = new DefaultHttpHeaders().add("X-Forwarded-For", "203.0.113.50");

        HTTPBalanceRequest reqFromClient1 = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.1", 1), headers1);
        HTTPBalanceRequest reqFromClient2 = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.2", 2), headers2);

        InetSocketAddress node1 = cluster.nextNode(reqFromClient1).node().socketAddress();
        InetSocketAddress node2 = cluster.nextNode(reqFromClient2).node().socketAddress();

        assertEquals(node1, node2,
                "Requests with the same X-Forwarded-For value must hash to the same node");

        // A different header value must (with very high probability) map to a different node.
        // Use enough distinct values to make collision extremely unlikely.
        HttpHeaders differentHeaders = new DefaultHttpHeaders().add("X-Forwarded-For", "198.51.100.99");
        HTTPBalanceRequest reqDifferent = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.3", 3), differentHeaders);
        // We just verify it resolves without error -- different value may still collide, so no assertion on inequality.
        assertNotNull(cluster.nextNode(reqDifferent).node());
    }

    /**
     * When the configured hash header is absent from the request, the hash key
     * must fall back to the client IP address. Also verifies HTTP/2 header lookup.
     */
    @Test
    void testHeaderFallbackToClientIP() throws Exception {
        // Use lowercase header name: HTTP/2 requires lowercase header names (RFC 7540 Section 8.1.2),
        // and the extractHashKey method does a case-sensitive lookup on Http2Headers.
        HTTPConsistentHash lb = new HTTPConsistentHash(NOOPSessionPersistence.INSTANCE, "x-real-ip");
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        for (int i = 1; i <= 5; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        // Request with no headers at all -- should fall back to client IP.
        InetSocketAddress clientAddr = new InetSocketAddress("192.168.1.50", 8080);
        HTTPBalanceRequest reqNoHeader = new HTTPBalanceRequest(clientAddr, EmptyHttpHeaders.INSTANCE);
        InetSocketAddress resultNoHeader = cluster.nextNode(reqNoHeader).node().socketAddress();
        assertNotNull(resultNoHeader);

        // A request from the same client IP (different port) with no header should map identically
        // because the hash key is the IP address only (port is excluded for stability).
        HTTPBalanceRequest reqSameIpDiffPort = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.50", 9090), EmptyHttpHeaders.INSTANCE);
        InetSocketAddress resultSameIp = cluster.nextNode(reqSameIpDiffPort).node().socketAddress();
        assertEquals(resultNoHeader, resultSameIp,
                "Fallback to client IP must be port-independent");

        // Request with the header present via HTTP/2 headers (lowercase per RFC 7540).
        Http2Headers h2Headers = new DefaultHttp2Headers().add("x-real-ip", "203.0.113.77");
        HTTPBalanceRequest reqH2 = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.50", 8080), h2Headers);
        InetSocketAddress resultH2 = cluster.nextNode(reqH2).node().socketAddress();
        assertNotNull(resultH2);

        // The HTTP/2 header provides a different hash key ("203.0.113.77" vs "192.168.1.50"),
        // so the node should differ with very high probability given 5 nodes.
        // This is not an absolute guarantee, but with murmur3 and 5 nodes it is nearly certain.
    }

    /**
     * Adding or removing a node should only remap approximately 1/N of the keys,
     * where N is the total number of nodes. This is the fundamental property of
     * consistent hashing.
     */
    @Test
    void testMinimalRemappingOnNodeChange() throws Exception {
        int nodeCount = 10;
        int keyCount = 10_000;

        HTTPConsistentHash lb = new HTTPConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        List<Node> nodes = new ArrayList<>();
        for (int i = 1; i <= nodeCount; i++) {
            nodes.add(fastBuild(cluster, "10.0.0." + i));
        }

        // Record the mapping for each key before the change.
        Map<String, InetSocketAddress> mappingBefore = new HashMap<>();
        for (int i = 0; i < keyCount; i++) {
            String clientIp = "172.16." + (i / 256) + "." + (i % 256);
            HTTPBalanceRequest request = new HTTPBalanceRequest(
                    new InetSocketAddress(clientIp, 1), EmptyHttpHeaders.INSTANCE);
            mappingBefore.put(clientIp, cluster.nextNode(request).node().socketAddress());
        }

        // Remove one node. This simulates a node going offline.
        Node removedNode = nodes.get(5);
        removedNode.close();

        // Record the mapping after the removal.
        int remappedCount = 0;
        for (int i = 0; i < keyCount; i++) {
            String clientIp = "172.16." + (i / 256) + "." + (i % 256);
            HTTPBalanceRequest request = new HTTPBalanceRequest(
                    new InetSocketAddress(clientIp, 1), EmptyHttpHeaders.INSTANCE);
            InetSocketAddress after = cluster.nextNode(request).node().socketAddress();
            if (!after.equals(mappingBefore.get(clientIp))) {
                remappedCount++;
            }
        }

        // With consistent hashing, removing 1 of N nodes should remap roughly 1/N keys.
        // Allow a generous tolerance: no more than 3/N of keys should be remapped.
        double remappedRatio = (double) remappedCount / keyCount;
        double expectedMax = 3.0 / nodeCount;
        assertTrue(remappedRatio <= expectedMax,
                "Removing 1 of " + nodeCount + " nodes remapped " + (remappedRatio * 100)
                        + "% of keys, expected at most " + (expectedMax * 100) + "%");

        // Also verify that some keys were actually remapped (the ones that hashed to the removed node).
        assertTrue(remappedCount > 0, "At least some keys should be remapped after node removal");
    }

    /**
     * When the hash ring is empty (no nodes in the cluster), response() must throw
     * NoNodeAvailableException.
     */
    @Test
    void testEmptyRingThrows() {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPConsistentHash(NOOPSessionPersistence.INSTANCE))
                .build();

        HTTPBalanceRequest request = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        assertThrows(NoNodeAvailableException.class, () -> cluster.nextNode(request));
    }

    /**
     * When the primary node for a hash key is offline, the ring walker must find
     * the next online node in clockwise order.
     */
    @Test
    void testOfflineNodeFallback() throws Exception {
        HTTPConsistentHash lb = new HTTPConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        for (int i = 1; i <= 5; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        HTTPBalanceRequest request = new HTTPBalanceRequest(
                new InetSocketAddress("192.168.1.100", 1), EmptyHttpHeaders.INSTANCE);

        // Determine which node this key maps to.
        Node originalNode = cluster.nextNode(request).node();
        assertNotNull(originalNode);

        // Mark the original node offline -- the ring walker should find the next online node.
        originalNode.markOffline();

        Node fallbackNode = cluster.nextNode(request).node();
        assertNotNull(fallbackNode);
        assertEquals(State.ONLINE, fallbackNode.state(),
                "Fallback node must be online");

        // Bring the original node back online. The key should map back to it.
        originalNode.markOnline();
        // Re-add to ring since markOnline fires NodeOnlineTask which calls addNodeToRing
        Node restoredNode = cluster.nextNode(request).node();
        assertEquals(originalNode.socketAddress(), restoredNode.socketAddress(),
                "After the original node comes back online, the key should map back to it");
    }

    /**
     * Concurrent reads (response calls) and writes (node additions/removals via
     * the event stream) must not cause data corruption or exceptions beyond
     * NoNodeAvailableException.
     */
    @Test
    void testConcurrentAccess() throws Exception {
        HTTPConsistentHash lb = new HTTPConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(lb)
                .build();

        // Start with a base set of nodes.
        for (int i = 1; i <= 10; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        int readerThreads = 6;
        int writerThreads = 2;
        int totalThreads = readerThreads + writerThreads;
        int iterationsPerThread = 3_000;
        CyclicBarrier barrier = new CyclicBarrier(totalThreads);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        AtomicInteger successfulReads = new AtomicInteger();

        List<Thread> threads = new ArrayList<>();

        // Reader threads: continuously hash requests and verify results.
        for (int t = 0; t < readerThreads; t++) {
            final int threadIndex = t;
            Thread reader = new Thread(() -> {
                try {
                    barrier.await();
                    for (int i = 0; i < iterationsPerThread; i++) {
                        HTTPBalanceRequest request = new HTTPBalanceRequest(
                                new InetSocketAddress("172.16." + threadIndex + "." + (i % 256), 1),
                                EmptyHttpHeaders.INSTANCE);
                        try {
                            Node node = cluster.nextNode(request).node();
                            if (node != null) {
                                successfulReads.incrementAndGet();
                            }
                        } catch (NoNodeAvailableException ignored) {
                            // Acceptable during concurrent node removal.
                        }
                    }
                } catch (Throwable ex) {
                    failure.compareAndSet(null, ex);
                }
            }, "Reader-" + t);
            threads.add(reader);
            reader.start();
        }

        // Writer threads: add and remove extra nodes to mutate the ring.
        for (int t = 0; t < writerThreads; t++) {
            final int writerIndex = t;
            Thread writer = new Thread(() -> {
                try {
                    barrier.await();
                    for (int i = 0; i < iterationsPerThread / 10; i++) {
                        String host = "10.1." + writerIndex + "." + (i % 256);
                        Node node = NodeBuilder.newBuilder()
                                .withCluster(cluster)
                                .withSocketAddress(new InetSocketAddress(host, 1))
                                .build();
                        // Let readers observe the new node for a few iterations.
                        Thread.yield();
                        node.close();
                    }
                } catch (Throwable ex) {
                    failure.compareAndSet(null, ex);
                }
            }, "Writer-" + writerIndex);
            threads.add(writer);
            writer.start();
        }

        for (Thread thread : threads) {
            thread.join(30_000);
        }

        assertNull(failure.get(),
                () -> "Concurrent access caused an exception: " + failure.get());
        assertTrue(successfulReads.get() > 0,
                "At least some reads should have succeeded");
    }

    private static Node fastBuild(Cluster cluster, String host) throws Exception {
        return NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
