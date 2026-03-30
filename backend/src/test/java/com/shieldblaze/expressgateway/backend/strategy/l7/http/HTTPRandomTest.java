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

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTPRandomTest {

    @Test
    void testRandom() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRandom(NOOPSessionPersistence.INSTANCE))
                .build();

        fastBuild(cluster, "172.16.20.1");
        fastBuild(cluster, "172.16.20.2");
        fastBuild(cluster, "172.16.20.3");
        fastBuild(cluster, "172.16.20.4");
        fastBuild(cluster, "172.16.20.5");

        HTTPBalanceRequest httpBalanceRequest = new HTTPBalanceRequest(new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;
        int fifth = 0;

        for (int i = 0; i < 1000; i++) {
            switch (cluster.nextNode(httpBalanceRequest).node().socketAddress().getHostString()) {
                case "172.16.20.1" -> {
                    first++;
                }
                case "172.16.20.2" -> {
                    second++;
                }
                case "172.16.20.3" -> {
                    third++;
                }
                case "172.16.20.4" -> {
                    forth++;
                }
                case "172.16.20.5" -> {
                    fifth++;
                }
                default -> {
                }
            }
        }

        assertTrue(first > 10);
        assertTrue(second > 10);
        assertTrue(third > 10);
        assertTrue(forth > 10);
        assertTrue(fifth > 10);
    }

    /**
     * BUG-HTTPRANDOM-THREAD: Verify that HTTPRandom.response() is safe to call
     * concurrently from multiple threads. The old SplittableRandom field had no
     * synchronization; under contention it could produce out-of-range indices
     * (ArrayIndexOutOfBoundsException) or corrupt internal state. ThreadLocalRandom
     * eliminates this by maintaining per-thread state with zero contention.
     *
     * <p>Test strategy: N threads all hammer response() through a CyclicBarrier to
     * maximize thread interleaving. Any exception is captured and surfaced as a
     * test failure. We also verify that every returned node belongs to the known
     * backend set, catching corrupted random values that might silently pick a
     * wrong index.
     */
    @Test
    void testThreadSafety() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRandom(NOOPSessionPersistence.INSTANCE))
                .build();

        List<String> hosts = List.of(
                "172.16.20.1", "172.16.20.2", "172.16.20.3",
                "172.16.20.4", "172.16.20.5"
        );
        for (String host : hosts) {
            fastBuild(cluster, host);
        }

        int threadCount = 8;
        int iterationsPerThread = 5_000;
        CyclicBarrier barrier = new CyclicBarrier(threadCount);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        AtomicInteger totalSelections = new AtomicInteger();

        List<Thread> threads = new ArrayList<>(threadCount);
        for (int t = 0; t < threadCount; t++) {
            // Each thread gets a unique source address so NOOPSessionPersistence
            // always returns null and we always hit the ThreadLocalRandom path.
            final int threadIndex = t;
            Thread thread = new Thread(() -> {
                try {
                    HTTPBalanceRequest request = new HTTPBalanceRequest(
                            new InetSocketAddress("10.0.0." + (threadIndex + 1), 1),
                            EmptyHttpHeaders.INSTANCE
                    );
                    barrier.await(); // maximize thread interleaving at start
                    for (int i = 0; i < iterationsPerThread; i++) {
                        Response response = cluster.nextNode(request);
                        String selectedHost = response.node().socketAddress().getHostString();
                        if (!hosts.contains(selectedHost)) {
                            throw new AssertionError(
                                    "ThreadLocalRandom returned unknown host: " + selectedHost);
                        }
                        totalSelections.incrementAndGet();
                    }
                } catch (Throwable ex) {
                    failure.compareAndSet(null, ex);
                }
            }, "HTTPRandom-test-" + t);
            threads.add(thread);
            thread.start();
        }

        for (Thread thread : threads) {
            thread.join(30_000); // 30s timeout to avoid hanging tests
        }

        // Surface the first failure with full stack trace
        assertNull(failure.get(),
                () -> "Concurrent access caused an exception: " + failure.get());

        // Verify all iterations completed (no silent ArrayIndexOutOfBounds etc.)
        assertTrue(totalSelections.get() == threadCount * iterationsPerThread,
                "Expected " + (threadCount * iterationsPerThread)
                        + " selections but got " + totalSelections.get());
    }

    private static void fastBuild(Cluster cluster, String host) throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
