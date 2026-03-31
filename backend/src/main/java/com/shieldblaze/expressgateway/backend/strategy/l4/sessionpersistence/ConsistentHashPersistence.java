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
package com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence;

import com.google.common.hash.HashFunction;
import com.google.common.hash.Hashing;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.locks.ReadWriteLock;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * <p> Consistent hash-based {@link SessionPersistence}. </p>
 *
 * <p> Instead of storing explicit session-to-node mappings (like {@link SourceIPHash}
 * or {@link FourTupleHash}), this implementation uses a hash ring to deterministically
 * map client addresses to backend nodes. The mapping is derived purely from the hash
 * ring state, so there is no per-session storage and no risk of unbounded memory growth. </p>
 *
 * <p> The trade-off is that session affinity is maintained only as long as the ring
 * remains stable. When a node is added or removed, some sessions will be remapped to
 * different nodes. With 150 virtual nodes per real node, approximately {@code 1/N}
 * sessions are affected (where N is the number of nodes). </p>
 *
 * <p> Hash key: The client's source IP address is used as the hash key (port is
 * excluded). This ensures that reconnections from the same client IP are routed to
 * the same backend, which is the same behavior as {@link SourceIPHash} but without
 * maintaining an explicit mapping table. </p>
 *
 * <p> Thread safety: A {@link ReadWriteLock} protects the hash ring. Read operations
 * (the hot path) acquire only the read lock and can proceed concurrently. </p>
 */
public final class ConsistentHashPersistence implements SessionPersistence<Node, Node, InetSocketAddress, Node> {

    /**
     * Number of virtual nodes per real node.
     */
    private static final int VIRTUAL_NODES = 150;

    private static final HashFunction HASH_FUNCTION = Hashing.murmur3_128();

    /**
     * The hash ring. TreeMap provides O(log n) ceiling lookups.
     */
    private final TreeMap<Long, Node> ring = new TreeMap<>();

    /**
     * ReadWriteLock for the hash ring.
     */
    private final ReadWriteLock ringLock = new ReentrantReadWriteLock();

    @Override
    public Node node(Request request) {
        String hashKey = request.socketAddress().getAddress().getHostAddress();
        long hash = hash(hashKey);

        ringLock.readLock().lock();
        try {
            if (ring.isEmpty()) {
                return null;
            }

            Map.Entry<Long, Node> entry = ring.ceilingEntry(hash);
            if (entry == null) {
                entry = ring.firstEntry();
            }
            return entry.getValue();
        } finally {
            ringLock.readLock().unlock();
        }
    }

    @Override
    public Node addRoute(InetSocketAddress socketAddress, Node node) {
        // The node is already on the ring (added via addNodeToRing during cluster events).
        // This method is called by the load balancer after selection on every request.
        // For consistent hash persistence, the ring IS the routing table — no per-session
        // state to store. Skipping addNodeToRing() avoids acquiring a write lock on every
        // request, which would serialize all concurrent lookups behind 150 TreeMap inserts.
        return node;
    }

    @Override
    public boolean removeRoute(InetSocketAddress socketAddress, Node node) {
        // Consistent hash persistence does not store per-session routes.
        // Removing a "route" means removing the node from the ring.
        removeNodeFromRing(node);
        return true;
    }

    @Override
    public boolean remove(Node nodeToRemove) {
        removeNodeFromRing(nodeToRemove);
        return true;
    }

    @Override
    public void clear() {
        ringLock.writeLock().lock();
        try {
            ring.clear();
        } finally {
            ringLock.writeLock().unlock();
        }
    }

    @Override
    public String name() {
        return "ConsistentHashPersistence";
    }

    @Override
    public String toString() {
        return "ConsistentHashPersistence{" +
                "ringSize=" + ring.size() +
                '}';
    }

    /**
     * Add a {@link Node} to the hash ring with its virtual nodes.
     * If the node's virtual node positions are already occupied by this node,
     * the operation is idempotent.
     *
     * @param node the node to add
     */
    private void addNodeToRing(Node node) {
        ringLock.writeLock().lock();
        try {
            for (int i = 0; i < VIRTUAL_NODES; i++) {
                long h = hash(virtualNodeKey(node, i));
                ring.putIfAbsent(h, node);
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
                long h = hash(virtualNodeKey(node, i));
                ring.remove(h, node);
            }
        } finally {
            ringLock.writeLock().unlock();
        }
    }

    private static String virtualNodeKey(Node node, int index) {
        return node.socketAddress().toString() + "#VN" + index;
    }

    private static long hash(String key) {
        return HASH_FUNCTION.hashString(key, StandardCharsets.UTF_8).asLong();
    }
}
