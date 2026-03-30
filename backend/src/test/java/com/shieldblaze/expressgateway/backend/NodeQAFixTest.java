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
package com.shieldblaze.expressgateway.backend;

import com.google.gson.JsonObject;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import io.netty.channel.ChannelFuture;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for QA review fixes on {@link Node}:
 * - decActiveConnection0 floor check (never goes negative)
 * - volatile maxConnections visibility across threads
 * - toJson uses AtomicLong.get() instead of raw AtomicLong
 * - synchronized instead of ReentrantLock (virtual thread pinning)
 */
class NodeQAFixTest {

    // -----------------------------------------------------------------------
    // 1. decActiveConnection0 floor check
    // -----------------------------------------------------------------------

    @Test
    void decActiveConnection0DoesNotGoNegative() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(9200))
                .build();

        // Start at zero, decrement 10 times
        for (int i = 0; i < 10; i++) {
            node.decActiveConnection0();
        }

        assertEquals(0, node.activeConnection0(),
                "activeConnection0 must not go negative");

        // Increment once, then decrement twice
        node.incActiveConnection0();
        assertEquals(1, node.activeConnection0());
        node.decActiveConnection0();
        assertEquals(0, node.activeConnection0());
        node.decActiveConnection0();
        assertEquals(0, node.activeConnection0(),
                "Decrement below zero must be a no-op");

        cluster.close();
    }

    @Test
    void decActiveConnection0ConcurrentFloorCheck() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(9201))
                .build();

        // Increment 100 times
        for (int i = 0; i < 100; i++) {
            node.incActiveConnection0();
        }

        // Decrement 200 times concurrently (100 more than incremented)
        int threadCount = 8;
        int decrementsPer = 25;
        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch latch = new CountDownLatch(threadCount);

        for (int t = 0; t < threadCount; t++) {
            executor.submit(() -> {
                for (int i = 0; i < decrementsPer; i++) {
                    node.decActiveConnection0();
                }
                latch.countDown();
            });
        }

        assertTrue(latch.await(10, TimeUnit.SECONDS));
        executor.shutdown();

        assertEquals(0, node.activeConnection0(),
                "After 100 increments and 200 decrements, counter must be 0 (not negative)");

        cluster.close();
    }

    // -----------------------------------------------------------------------
    // 2. volatile maxConnections visibility
    // -----------------------------------------------------------------------

    @Test
    void maxConnectionsVolatileVisibility() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(9202))
                .build();

        // Writer thread sets maxConnections
        AtomicInteger observed = new AtomicInteger(0);
        CountDownLatch ready = new CountDownLatch(1);
        CountDownLatch done = new CountDownLatch(1);

        Thread.ofVirtual().start(() -> {
            ready.countDown();
            // Spin until we see the updated value
            while (node.maxConnections() != 42_000) {
                Thread.onSpinWait();
            }
            observed.set(node.maxConnections());
            done.countDown();
        });

        ready.await();
        // Small delay to ensure reader is spinning
        Thread.sleep(10);
        node.maxConnections(42_000);

        assertTrue(done.await(5, TimeUnit.SECONDS),
                "Reader thread must see the volatile maxConnections update");
        assertEquals(42_000, observed.get());

        cluster.close();
    }

    // -----------------------------------------------------------------------
    // 3. toJson uses .get() on AtomicLong fields
    // -----------------------------------------------------------------------

    @Test
    void toJsonReturnsNumericByteCounts() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(9203))
                .build();

        // Increment byte counters
        node.incBytesSent(1024);
        node.incBytesReceived(2048);

        JsonObject json = node.toJson();

        // The fix ensures these are numbers (long), not stringified AtomicLong objects
        assertTrue(json.get("BytesSent").isJsonPrimitive(),
                "BytesSent must be a JSON primitive (number)");
        assertTrue(json.get("BytesReceived").isJsonPrimitive(),
                "BytesReceived must be a JSON primitive (number)");
        assertEquals(1024L, json.get("BytesSent").getAsLong());
        assertEquals(2048L, json.get("BytesReceived").getAsLong());

        cluster.close();
    }

    // -----------------------------------------------------------------------
    // 4. synchronized addConnection/removeConnection (replaces ReentrantLock)
    // -----------------------------------------------------------------------

    @Test
    void synchronizedAddRemoveConnectionConcurrency() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        Node node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(9204))
                .build();
        node.maxConnections(Integer.MAX_VALUE);

        int threadCount = 8;
        int connectionsPerThread = 500;
        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        CountDownLatch start = new CountDownLatch(1);
        CountDownLatch done = new CountDownLatch(threadCount);
        DummyConnection[][] allConnections = new DummyConnection[threadCount][connectionsPerThread];

        // Concurrently add connections
        for (int t = 0; t < threadCount; t++) {
            final int tid = t;
            executor.submit(() -> {
                try {
                    start.await();
                    for (int i = 0; i < connectionsPerThread; i++) {
                        DummyConnection conn = new DummyConnection(node);
                        allConnections[tid][i] = conn;
                        node.addConnection(conn);
                    }
                } catch (Exception e) {
                    throw new RuntimeException(e);
                } finally {
                    done.countDown();
                }
            });
        }

        start.countDown();
        assertTrue(done.await(10, TimeUnit.SECONDS));

        assertEquals(threadCount * connectionsPerThread, node.activeConnection(),
                "All connections must be added");

        // Concurrently remove all connections
        CountDownLatch removeDone = new CountDownLatch(threadCount);
        for (int t = 0; t < threadCount; t++) {
            final int tid = t;
            executor.submit(() -> {
                for (int i = 0; i < connectionsPerThread; i++) {
                    node.removeConnection(allConnections[tid][i]);
                }
                removeDone.countDown();
            });
        }

        assertTrue(removeDone.await(10, TimeUnit.SECONDS));
        executor.shutdown();

        assertEquals(0, node.activeConnection(),
                "After removing all connections, count must be 0");

        cluster.close();
    }

    private static final class DummyConnection extends Connection {
        private DummyConnection(Node node) {
            super(node);
        }

        @Override
        protected void processBacklog(ChannelFuture channelFuture) {
            // No-op
        }
    }
}
