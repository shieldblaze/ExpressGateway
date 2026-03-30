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
package com.shieldblaze.expressgateway.backend.strategy.l4;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class RoundRobinTest {

    @Test
    void testRoundRobin() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE)).build();

        // Add Node Server Addresses
        for (int i = 1; i <= 100; i++) {
            fastBuild(cluster, "192.168.1." + i);
        }

        L4Request l4Request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        for (int i = 1; i <= 100; i++) {
            assertEquals(new InetSocketAddress("192.168.1." + i, 1), cluster.nextNode(l4Request).node().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertEquals(new InetSocketAddress("192.168.1." + i, 1), cluster.nextNode(l4Request).node().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(new InetSocketAddress("10.10.1." + i, 1), cluster.nextNode(l4Request).node().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(new InetSocketAddress("172.16.20." + i, 1), cluster.nextNode(l4Request).node().socketAddress());
        }
    }

    /**
     * Verifies that concurrent calls to response() do not cause
     * IndexOutOfBoundsException and always return valid (non-null) nodes.
     *
     * This exercises the AtomicInteger-based CAS loop in RoundRobinIndexGenerator
     * and the modular arithmetic against onlineNodes.size() under contention.
     */
    @Test
    void testRoundRobinConcurrency() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        // Add 10 backend nodes
        for (int i = 1; i <= 10; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        int threadCount = 8;
        int iterationsPerThread = 5_000;
        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);
        List<Throwable> errors = new CopyOnWriteArrayList<>();
        AtomicInteger successCount = new AtomicInteger(0);

        for (int t = 0; t < threadCount; t++) {
            final int threadId = t;
            executor.submit(() -> {
                try {
                    // Wait for all threads to be ready before starting
                    startLatch.await();

                    for (int i = 0; i < iterationsPerThread; i++) {
                        L4Request request = new L4Request(
                                new InetSocketAddress("192.168." + threadId + "." + (i % 256), 1));
                        var response = cluster.nextNode(request);
                        assertNotNull(response, "Response must not be null");
                        assertNotNull(response.node(), "Response node must not be null");
                        assertNotNull(response.node().socketAddress(), "Node socket address must not be null");
                        successCount.incrementAndGet();
                    }
                } catch (Throwable e) {
                    errors.add(e);
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        // Release all threads simultaneously for maximum contention
        startLatch.countDown();
        assertTrue(doneLatch.await(30, TimeUnit.SECONDS), "Test timed out");
        executor.shutdown();

        // Verify no errors occurred (especially IndexOutOfBoundsException)
        assertTrue(errors.isEmpty(),
                "Concurrent RoundRobin produced errors: " + errors);
        assertEquals(threadCount * iterationsPerThread, successCount.get(),
                "All iterations must complete successfully");

        // Close Cluster to prevent memory leaks.
        cluster.close();
    }

    private static void fastBuild(Cluster cluster, String host) throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
