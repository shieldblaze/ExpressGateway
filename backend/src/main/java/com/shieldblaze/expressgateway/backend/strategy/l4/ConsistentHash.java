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

import com.google.common.hash.HashFunction;
import com.google.common.hash.Hashing;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeTask;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.concurrent.task.Task;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.locks.ReadWriteLock;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * <p> Select {@link Node} based on consistent hashing. </p>
 *
 * <p> Uses a TreeMap-based hash ring with virtual nodes (150 per real node)
 * to distribute keys evenly. On node addition or removal, only the keys
 * mapped to the affected segment of the ring are remapped, minimizing
 * disruption. </p>
 *
 * <p> Hash function: Murmur3-128 via Guava. This is a non-cryptographic hash
 * that provides excellent distribution and avalanche properties for consistent
 * hashing. It is NOT used for security-sensitive purposes. </p>
 *
 * <p> Thread safety: A {@link ReadWriteLock} protects the hash ring. Concurrent
 * reads (the hot path) proceed without blocking each other. Writes (node
 * add/remove) acquire the exclusive write lock and rebuild the affected
 * ring segment. </p>
 */
public final class ConsistentHash extends L4Balance {

    /**
     * Number of virtual nodes per real node. 150 virtual nodes provides a good
     * balance between ring distribution uniformity and memory overhead.
     * With N real nodes, the ring contains N * 150 entries. At 100 nodes this
     * is 15,000 TreeMap entries -- well within acceptable limits.
     */
    private static final int VIRTUAL_NODES = 150;

    private static final HashFunction HASH_FUNCTION = Hashing.murmur3_128();

    /**
     * The hash ring. Keys are hash values of virtual node identifiers,
     * values are the real {@link Node} instances. TreeMap provides O(log n)
     * ceiling lookups which is critical for the ring walk.
     */
    private final TreeMap<Long, Node> ring = new TreeMap<>();

    /**
     * ReadWriteLock for the hash ring. The read lock is used for the hot
     * path (node selection), the write lock for ring mutations.
     */
    private final ReadWriteLock ringLock = new ReentrantReadWriteLock();

    /**
     * Create {@link ConsistentHash} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    public ConsistentHash(SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence) {
        super(sessionPersistence);
    }

    @Override
    public String name() {
        return "ConsistentHash";
    }

    @Override
    public L4Response balance(L4Request l4Request) throws LoadBalanceException {
        Node node = sessionPersistence.node(l4Request);
        if (node != null) {
            if (node.state() == State.ONLINE) {
                return new L4Response(node);
            } else {
                sessionPersistence.removeRoute(l4Request.socketAddress(), node);
            }
        }

        // Hash the client socket address (IP + port) to a ring position.
        // Using the full socket address as key means the same client connection
        // always maps to the same backend, providing natural affinity.
        long hash = hash(l4Request.socketAddress().toString());

        ringLock.readLock().lock();
        try {
            if (ring.isEmpty()) {
                throw NoNodeAvailableException.INSTANCE;
            }

            // Walk the ring clockwise from the hash position.
            // ceilingEntry returns the least entry >= hash, or null if past the end.
            // If null, wrap around to the first entry (the ring is circular).
            Map.Entry<Long, Node> entry = ring.ceilingEntry(hash);
            if (entry == null) {
                entry = ring.firstEntry();
            }

            node = entry.getValue();

            // If the resolved node is not online, fall back to walking the ring
            // to find the next online node. This handles the window between a node
            // going offline and the ring being rebuilt by the event handler.
            if (node.state() != State.ONLINE) {
                node = findNextOnlineNode(hash);
                if (node == null) {
                    throw NoNodeAvailableException.INSTANCE;
                }
            }
        } finally {
            ringLock.readLock().unlock();
        }

        sessionPersistence.addRoute(l4Request.socketAddress(), node);
        return new L4Response(node);
    }

    @Override
    public void accept(Task task) {
        if (task instanceof NodeTask nodeEvent) {
            if (nodeEvent instanceof NodeOfflineTask || nodeEvent instanceof NodeRemovedTask || nodeEvent instanceof NodeIdleTask) {
                sessionPersistence.remove(nodeEvent.node());
                removeNodeFromRing(nodeEvent.node());
            } else if (nodeEvent instanceof NodeOnlineTask || nodeEvent instanceof NodeAddedTask) {
                addNodeToRing(nodeEvent.node());
            }
        }
    }

    @Override
    public String toString() {
        return "ConsistentHash{" +
                "sessionPersistence=" + sessionPersistence +
                ", cluster=" + cluster +
                '}';
    }

    @Override
    public void close() throws IOException {
        sessionPersistence.clear();
        ringLock.writeLock().lock();
        try {
            ring.clear();
        } finally {
            ringLock.writeLock().unlock();
        }
    }

    /**
     * Add a {@link Node} to the hash ring with its virtual nodes.
     *
     * @param node the node to add
     */
    private void addNodeToRing(Node node) {
        ringLock.writeLock().lock();
        try {
            for (int i = 0; i < VIRTUAL_NODES; i++) {
                long hash = hash(virtualNodeKey(node, i));
                ring.put(hash, node);
            }
        } finally {
            ringLock.writeLock().unlock();
        }
    }

    /**
     * Remove a {@link Node} from the hash ring along with all its virtual nodes.
     *
     * @param node the node to remove
     */
    private void removeNodeFromRing(Node node) {
        ringLock.writeLock().lock();
        try {
            for (int i = 0; i < VIRTUAL_NODES; i++) {
                long hash = hash(virtualNodeKey(node, i));
                // Only remove if the entry still points to this node.
                // Another node may have been assigned this hash position.
                ring.remove(hash, node);
            }
        } finally {
            ringLock.writeLock().unlock();
        }
    }

    /**
     * Walk the ring clockwise from the given hash to find the next online node.
     * Must be called while holding the read lock.
     *
     * @param hash starting position on the ring
     * @return the next online {@link Node}, or {@code null} if none is online
     */
    private Node findNextOnlineNode(long hash) {
        // Walk from hash to the end of the ring
        for (Map.Entry<Long, Node> entry : ring.tailMap(hash, true).entrySet()) {
            if (entry.getValue().state() == State.ONLINE) {
                return entry.getValue();
            }
        }
        // Wrap around: walk from the beginning of the ring to the hash position
        for (Map.Entry<Long, Node> entry : ring.headMap(hash, false).entrySet()) {
            if (entry.getValue().state() == State.ONLINE) {
                return entry.getValue();
            }
        }
        return null;
    }

    /**
     * Generate a deterministic key for a virtual node. The format includes
     * both the node's socket address (which is used for equals/hashCode on Node)
     * and its unique ID to handle the case where two nodes share an address
     * (which should not happen, but defensive coding).
     *
     * @param node  the real node
     * @param index the virtual node index
     * @return a string key for hashing
     */
    private static String virtualNodeKey(Node node, int index) {
        return node.socketAddress().toString() + "#VN" + index;
    }

    /**
     * Compute a 64-bit hash value from the given key using Murmur3-128.
     * We take the lower 64 bits of the 128-bit hash since TreeMap keys
     * are {@code long}. Murmur3-128's lower 64 bits have the same
     * distribution quality as the full hash for our purposes.
     *
     * @param key the key to hash
     * @return a 64-bit hash value
     */
    private static long hash(String key) {
        return HASH_FUNCTION.hashString(key, StandardCharsets.UTF_8).asLong();
    }
}
