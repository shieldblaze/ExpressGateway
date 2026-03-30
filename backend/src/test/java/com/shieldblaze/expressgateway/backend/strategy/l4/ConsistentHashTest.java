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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedTask;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConsistentHashTest {

    /**
     * The same client address must always resolve to the same backend node.
     * This is the fundamental invariant of consistent hashing: deterministic
     * mapping from key to node when the ring is stable.
     */
    @Test
    void testBasicConsistentHashing() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new ConsistentHash(NOOPSessionPersistence.INSTANCE))
                .build();

        for (int i = 1; i <= 10; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        // Issue the same request 100 times -- must always land on the same node.
        L4Request request = new L4Request(new InetSocketAddress("192.168.1.100", 5000));
        Node firstResult = cluster.nextNode(request).node();
        assertNotNull(firstResult);

        for (int i = 0; i < 100; i++) {
            Node result = cluster.nextNode(request).node();
            assertSame(firstResult, result,
                    "Consistent hashing must return the same node for the same client address");
        }

        // A different client address should (with very high probability) map to a
        // different node, verifying that the hash ring actually distributes.
        // With 10 nodes the probability of collision is ~10%, so we test multiple
        // distinct addresses and require at least 2 distinct backend nodes.
        java.util.Set<Node> distinctTargets = new java.util.HashSet<>();
        for (int i = 1; i <= 20; i++) {
            L4Request other = new L4Request(new InetSocketAddress("172.16." + i + ".1", 6000));
            distinctTargets.add(cluster.nextNode(other).node());
        }
        assertTrue(distinctTargets.size() >= 2,
                "20 distinct client addresses should map to at least 2 different backend nodes");

        cluster.close();
    }

    /**
     * When the client hash is beyond the last entry on the ring, the ring must
     * wrap around to the first entry (circular ring property). We verify this
     * indirectly: every client address must resolve to a valid node, which can
     * only happen if wraparound works correctly for hashes that land past the
     * ring's maximum key.
     */
    @Test
    void testRingWraparound() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new ConsistentHash(NOOPSessionPersistence.INSTANCE))
                .build();

        // Use a small number of nodes so the ring is sparse and wraparound is
        // exercised by a significant fraction of client hashes.
        fastBuild(cluster, "10.0.0.1");
        fastBuild(cluster, "10.0.0.2");

        // Generate many diverse client addresses. Every one must resolve to one
        // of the two nodes -- if wraparound is broken, ceilingEntry returns null
        // and response() would throw NoNodeAvailableException or NPE.
        for (int i = 0; i < 500; i++) {
            L4Request request = new L4Request(
                    new InetSocketAddress("192.168." + (i / 256) + "." + (i % 256), i + 1));
            Node node = cluster.nextNode(request).node();
            assertNotNull(node, "Ring wraparound must always yield a valid node");
            assertTrue(
                    node.socketAddress().equals(new InetSocketAddress("10.0.0.1", 1)) ||
                    node.socketAddress().equals(new InetSocketAddress("10.0.0.2", 1)),
                    "Result must be one of the two backend nodes");
        }

        cluster.close();
    }

    /**
     * Removing one node from N nodes should remap approximately 1/N of the
     * keys. We verify that the vast majority of mappings remain stable.
     */
    @Test
    void testMinimalRemappingOnNodeRemoval() throws Exception {
        int nodeCount = 10;
        ConsistentHash ch = new ConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(ch).build();

        Node[] nodes = new Node[nodeCount];
        for (int i = 0; i < nodeCount; i++) {
            nodes[i] = fastBuild(cluster, "10.0.0." + (i + 1));
        }

        // Record mappings for a large sample of client addresses before removal.
        int sampleSize = 10_000;
        Map<InetSocketAddress, Node> beforeMapping = new HashMap<>();
        for (int i = 0; i < sampleSize; i++) {
            InetSocketAddress clientAddr = new InetSocketAddress("192.168." + (i / 256) + "." + (i % 256), 8000 + (i % 1000));
            L4Request request = new L4Request(clientAddr);
            beforeMapping.put(clientAddr, cluster.nextNode(request).node());
        }

        // Remove one node by sending a NodeRemovedTask directly to the load balancer.
        // We pick a middle node to avoid edge effects.
        Node removedNode = nodes[5];
        ch.accept(new NodeRemovedTask(removedNode));

        // Record mappings after removal.
        int remapped = 0;
        for (Map.Entry<InetSocketAddress, Node> entry : beforeMapping.entrySet()) {
            L4Request request = new L4Request(entry.getKey());
            Node afterNode = ch.balance(request).node();

            if (!afterNode.equals(entry.getValue())) {
                remapped++;
            }
        }

        // Theoretical: ~1/N keys remap = ~10%. Allow up to 25% for variance
        // due to virtual node distribution. At minimum, some keys must remap
        // (those that were on the removed node).
        double remapPercentage = (double) remapped / sampleSize * 100.0;
        assertTrue(remapPercentage < 25.0,
                "Removing 1 of " + nodeCount + " nodes should remap <25% of keys, but remapped " +
                String.format("%.1f%%", remapPercentage));
        assertTrue(remapped > 0, "At least some keys must be remapped after node removal");

        cluster.close();
    }

    /**
     * Adding one node to N existing nodes should remap approximately 1/(N+1)
     * of the keys. We verify that the vast majority of mappings remain stable.
     */
    @Test
    void testMinimalRemappingOnNodeAddition() throws Exception {
        int nodeCount = 10;
        ConsistentHash ch = new ConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(ch).build();

        for (int i = 0; i < nodeCount; i++) {
            fastBuild(cluster, "10.0.0." + (i + 1));
        }

        // Record mappings before addition.
        int sampleSize = 10_000;
        Map<InetSocketAddress, Node> beforeMapping = new HashMap<>();
        for (int i = 0; i < sampleSize; i++) {
            InetSocketAddress clientAddr = new InetSocketAddress("192.168." + (i / 256) + "." + (i % 256), 8000 + (i % 1000));
            L4Request request = new L4Request(clientAddr);
            beforeMapping.put(clientAddr, cluster.nextNode(request).node());
        }

        // Add one new node.
        fastBuild(cluster, "10.0.0.100");

        // Record mappings after addition.
        int remapped = 0;
        for (Map.Entry<InetSocketAddress, Node> entry : beforeMapping.entrySet()) {
            L4Request request = new L4Request(entry.getKey());
            Node afterNode = cluster.nextNode(request).node();

            if (!afterNode.equals(entry.getValue())) {
                remapped++;
            }
        }

        // Theoretical: ~1/(N+1) keys remap = ~9%. Allow up to 25% for variance.
        double remapPercentage = (double) remapped / sampleSize * 100.0;
        assertTrue(remapPercentage < 25.0,
                "Adding 1 node to " + nodeCount + " should remap <25% of keys, but remapped " +
                String.format("%.1f%%", remapPercentage));
        assertTrue(remapped > 0, "At least some keys must be remapped after node addition");

        cluster.close();
    }

    /**
     * When the ring is empty (no nodes added), response() must throw
     * {@link NoNodeAvailableException}.
     */
    @Test
    void testEmptyRingThrowsNoNodeAvailable() {
        ConsistentHash ch = new ConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(ch).build();

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1234));
        assertThrows(NoNodeAvailableException.class, () -> ch.balance(request));

        cluster.close();
    }

    /**
     * When the initially resolved node is offline (state != ONLINE but still on
     * the ring), the implementation must walk the ring to find the next online
     * node via {@code findNextOnlineNode}.
     *
     * We set the node's state directly (not via markOffline, which publishes
     * events that remove the node from the ring) to simulate the window between
     * a node going offline and the ring rebuild.
     */
    @Test
    void testFallbackToNextOnlineNode() throws Exception {
        ConsistentHash ch = new ConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(ch).build();

        Node node1 = fastBuild(cluster, "10.0.0.1");
        Node node2 = fastBuild(cluster, "10.0.0.2");
        Node node3 = fastBuild(cluster, "10.0.0.3");

        // Find a client address that maps to each node so we can test fallback.
        // Probe client addresses until we find one that maps to node1.
        InetSocketAddress clientForNode1 = null;
        for (int i = 0; i < 10_000; i++) {
            InetSocketAddress addr = new InetSocketAddress("172.16." + (i / 256) + "." + (i % 256), 9000);
            L4Request probe = new L4Request(addr);
            if (ch.balance(probe).node().equals(node1)) {
                clientForNode1 = addr;
                break;
            }
        }
        assertNotNull(clientForNode1, "Must find at least one client address that maps to node1");

        // Now mark node1 as OFFLINE directly (bypassing event system) so it remains
        // on the ring but is not ONLINE. The response() method should fall through
        // to findNextOnlineNode.
        node1.state(State.OFFLINE);

        L4Request request = new L4Request(clientForNode1);
        Node fallbackNode = ch.balance(request).node();
        assertNotNull(fallbackNode, "Fallback must find an online node");
        assertTrue(fallbackNode.state() == State.ONLINE,
                "Fallback node must be ONLINE");
        assertTrue(fallbackNode.equals(node2) || fallbackNode.equals(node3),
                "Fallback must return one of the remaining online nodes");

        cluster.close();
    }

    /**
     * When all nodes on the ring are offline (state != ONLINE), response() must
     * throw {@link NoNodeAvailableException}.
     */
    @Test
    void testAllNodesOfflineThrowsNoNodeAvailable() throws Exception {
        ConsistentHash ch = new ConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(ch).build();

        Node node1 = fastBuild(cluster, "10.0.0.1");
        Node node2 = fastBuild(cluster, "10.0.0.2");
        Node node3 = fastBuild(cluster, "10.0.0.3");

        // Mark all nodes offline directly (keeping them on the ring).
        node1.state(State.OFFLINE);
        node2.state(State.OFFLINE);
        node3.state(State.OFFLINE);

        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1234));
        assertThrows(NoNodeAvailableException.class, () -> ch.balance(request));

        cluster.close();
    }

    /**
     * Verify thread safety of concurrent reads (response) and writes (accept
     * with NodeAddedTask/NodeRemovedTask). The ReadWriteLock must prevent
     * ConcurrentModificationException and ensure every response() call either
     * returns a valid node or throws NoNodeAvailableException.
     */
    @Test
    void testConcurrentReadsAndWrites() throws Exception {
        ConsistentHash ch = new ConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(ch).build();

        // Pre-populate with some nodes so reads don't always hit an empty ring.
        for (int i = 1; i <= 5; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        int readerThreads = 4;
        int writerThreads = 2;
        int totalThreads = readerThreads + writerThreads;
        int iterationsPerThread = 5_000;
        ExecutorService executor = Executors.newFixedThreadPool(totalThreads);
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(totalThreads);
        List<Throwable> errors = new CopyOnWriteArrayList<>();
        AtomicInteger readSuccess = new AtomicInteger(0);
        AtomicInteger writeSuccess = new AtomicInteger(0);

        // Reader threads: issue response() calls with varying client addresses.
        for (int t = 0; t < readerThreads; t++) {
            final int threadId = t;
            executor.submit(() -> {
                try {
                    startLatch.await();
                    for (int i = 0; i < iterationsPerThread; i++) {
                        L4Request request = new L4Request(
                                new InetSocketAddress("192.168." + threadId + "." + (i % 256), 1));
                        try {
                            L4Response response = ch.balance(request);
                            assertNotNull(response, "Response must not be null");
                            assertNotNull(response.node(), "Response node must not be null");
                            readSuccess.incrementAndGet();
                        } catch (NoNodeAvailableException e) {
                            // Acceptable during ring mutations -- the ring may be
                            // momentarily empty if all nodes have been removed.
                            readSuccess.incrementAndGet();
                        }
                    }
                } catch (Throwable e) {
                    errors.add(e);
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        // Writer threads: add and remove nodes to mutate the ring.
        for (int t = 0; t < writerThreads; t++) {
            final int writerOffset = 100 + t * 50;
            executor.submit(() -> {
                try {
                    startLatch.await();
                    for (int i = 0; i < iterationsPerThread; i++) {
                        // Alternate between adding and removing a node.
                        Node tempNode = fastBuild(cluster, "10.0." + writerOffset + "." + (i % 256));
                        ch.accept(new NodeRemovedTask(tempNode));
                        writeSuccess.incrementAndGet();
                    }
                } catch (Throwable e) {
                    errors.add(e);
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        startLatch.countDown();
        assertTrue(doneLatch.await(60, TimeUnit.SECONDS), "Test timed out");
        executor.shutdown();

        assertTrue(errors.isEmpty(),
                "Concurrent reads/writes produced errors: " + errors);
        assertEquals(readerThreads * iterationsPerThread, readSuccess.get(),
                "All reader iterations must complete");
        assertEquals(writerThreads * iterationsPerThread, writeSuccess.get(),
                "All writer iterations must complete");

        cluster.close();
    }

    /**
     * Verify that accept() correctly handles the five node event types:
     * - NodeOnlineTask / NodeAddedTask  -> addNodeToRing
     * - NodeOfflineTask / NodeRemovedTask / NodeIdleTask -> removeNodeFromRing
     */
    @Test
    void testNodeEventHandling() throws Exception {
        ConsistentHash ch = new ConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(ch).build();

        Node node1 = fastBuild(cluster, "10.0.0.1");

        // At this point node1 is on the ring (NodeAddedTask was published by Cluster.addNode).
        // Verify it is reachable.
        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1234));
        Node result = ch.balance(request).node();
        assertSame(node1, result, "Only node on the ring must be returned");

        // Simulate NodeOfflineTask -- should remove node from ring.
        ch.accept(new NodeOfflineTask(node1));
        assertThrows(NoNodeAvailableException.class, () -> ch.balance(request),
                "Ring should be empty after NodeOfflineTask removes the only node");

        // Simulate NodeOnlineTask -- should re-add node to ring.
        ch.accept(new NodeOnlineTask(node1));
        result = ch.balance(request).node();
        assertSame(node1, result, "Node must be back on the ring after NodeOnlineTask");

        // Simulate NodeIdleTask -- should remove node from ring.
        ch.accept(new NodeIdleTask(node1));
        assertThrows(NoNodeAvailableException.class, () -> ch.balance(request),
                "Ring should be empty after NodeIdleTask removes the only node");

        // Simulate NodeAddedTask -- should re-add node to ring.
        ch.accept(new NodeAddedTask(node1));
        result = ch.balance(request).node();
        assertSame(node1, result, "Node must be back on the ring after NodeAddedTask");

        // Simulate NodeRemovedTask -- should remove node from ring.
        ch.accept(new NodeRemovedTask(node1));
        assertThrows(NoNodeAvailableException.class, () -> ch.balance(request),
                "Ring should be empty after NodeRemovedTask removes the only node");

        cluster.close();
    }

    /**
     * Verify that close() clears both the session persistence and the hash ring,
     * making subsequent response() calls throw NoNodeAvailableException.
     */
    @Test
    void testCloseCleanup() throws Exception {
        ConsistentHash ch = new ConsistentHash(NOOPSessionPersistence.INSTANCE);
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(ch).build();

        for (int i = 1; i <= 5; i++) {
            fastBuild(cluster, "10.0.0." + i);
        }

        // Verify the ring is populated and serving requests.
        L4Request request = new L4Request(new InetSocketAddress("192.168.1.1", 1234));
        assertNotNull(ch.balance(request).node(), "Ring must be populated before close");

        // Close the consistent hash -- ring and session persistence should be cleared.
        ch.close();

        // After close, the ring is empty so response() must throw.
        assertThrows(NoNodeAvailableException.class, () -> ch.balance(request),
                "Ring must be empty after close()");

        cluster.close();
    }

    private static Node fastBuild(Cluster cluster, String host) throws Exception {
        return NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
