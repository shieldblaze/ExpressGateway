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
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.locks.ReadWriteLock;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * <p> Select {@link Node} based on consistent hashing for HTTP traffic. </p>
 *
 * <p> The hash key is derived from either the client IP address or a configurable
 * HTTP header value. When a header name is configured, its value is used as the
 * hash key (e.g., {@code X-Forwarded-For}, {@code X-Real-IP}, or any custom header).
 * If the header is absent from the request, the client IP address is used as fallback. </p>
 *
 * <p> Uses the same TreeMap-based hash ring with 150 virtual nodes per real node
 * as the L4 {@link com.shieldblaze.expressgateway.backend.strategy.l4.ConsistentHash}
 * implementation. See that class for details on the ring algorithm. </p>
 *
 * <p> Thread safety: A {@link ReadWriteLock} protects the hash ring. Concurrent
 * reads proceed without blocking each other; writes acquire the exclusive lock. </p>
 */
public final class HTTPConsistentHash extends HTTPBalance {

    /**
     * Number of virtual nodes per real node.
     */
    private static final int VIRTUAL_NODES = 150;

    private static final HashFunction HASH_FUNCTION = Hashing.murmur3_128();

    /**
     * The hash ring. Keys are hash values, values are real {@link Node} instances.
     */
    private final TreeMap<Long, Node> ring = new TreeMap<>();

    /**
     * ReadWriteLock for the hash ring.
     */
    private final ReadWriteLock ringLock = new ReentrantReadWriteLock();

    /**
     * Optional header name to use as the hash key. When non-null, the value of
     * this header is used instead of the client IP address.
     */
    private final String hashHeaderName;

    /**
     * Create {@link HTTPConsistentHash} Instance using client IP as hash key.
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    public HTTPConsistentHash(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence) {
        this(sessionPersistence, null);
    }

    /**
     * Create {@link HTTPConsistentHash} Instance with a configurable hash key header.
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     * @param hashHeaderName     HTTP header name whose value is used as the hash key.
     *                           If {@code null}, the client IP address is used.
     */
    public HTTPConsistentHash(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence,
                              String hashHeaderName) {
        super(sessionPersistence);
        this.hashHeaderName = hashHeaderName;
    }

    @Override
    public String name() {
        return "HTTPConsistentHash";
    }

    @Override
    public HTTPBalanceResponse balance(HTTPBalanceRequest request) throws LoadBalanceException {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.node(request);
        if (httpBalanceResponse != null) {
            if (httpBalanceResponse.node().state() == State.ONLINE) {
                return httpBalanceResponse;
            } else {
                sessionPersistence.removeRoute(request, httpBalanceResponse.node());
            }
        }

        // Derive the hash key from the configured header or fallback to client IP.
        String hashKey = extractHashKey(request);
        long hash = hash(hashKey);

        Node node;
        ringLock.readLock().lock();
        try {
            if (ring.isEmpty()) {
                throw NoNodeAvailableException.INSTANCE;
            }

            Map.Entry<Long, Node> entry = ring.ceilingEntry(hash);
            if (entry == null) {
                entry = ring.firstEntry();
            }

            node = entry.getValue();

            // Fall back to walking the ring if the resolved node is offline.
            if (node.state() != State.ONLINE) {
                node = findNextOnlineNode(hash);
                if (node == null) {
                    throw NoNodeAvailableException.INSTANCE;
                }
            }
        } finally {
            ringLock.readLock().unlock();
        }

        return sessionPersistence.addRoute(request, node);
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
        return "HTTPConsistentHash{" +
                "hashHeaderName=" + hashHeaderName +
                ", sessionPersistence=" + sessionPersistence +
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
     * Extract the hash key from the request. If a header name is configured,
     * attempts to read it from HTTP/1.1 headers first, then HTTP/2 headers.
     * Falls back to the client IP address if the header is not present.
     *
     * @param request the HTTP balance request
     * @return the hash key string
     */
    private String extractHashKey(HTTPBalanceRequest request) {
        if (hashHeaderName != null) {
            // Try HTTP/1.1 headers first
            if (request.httpHeaders() != null) {
                String value = request.httpHeaders().get(hashHeaderName);
                if (value != null && !value.isEmpty()) {
                    return value;
                }
            }
            // Try HTTP/2 headers
            if (request.http2Headers() != null) {
                CharSequence value = request.http2Headers().get(hashHeaderName);
                if (value != null && value.length() > 0) {
                    return value.toString();
                }
            }
        }
        // Fallback: client IP address (without port for stability across connections)
        return request.socketAddress().getAddress().getHostAddress();
    }

    /**
     * Add a {@link Node} to the hash ring with its virtual nodes.
     */
    private void addNodeToRing(Node node) {
        ringLock.writeLock().lock();
        try {
            for (int i = 0; i < VIRTUAL_NODES; i++) {
                long h = hash(virtualNodeKey(node, i));
                ring.put(h, node);
            }
        } finally {
            ringLock.writeLock().unlock();
        }
    }

    /**
     * Remove a {@link Node} from the hash ring along with all its virtual nodes.
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

    /**
     * Walk the ring clockwise from the given hash to find the next online node.
     * Must be called while holding the read lock.
     */
    private Node findNextOnlineNode(long hash) {
        for (Map.Entry<Long, Node> entry : ring.tailMap(hash, true).entrySet()) {
            if (entry.getValue().state() == State.ONLINE) {
                return entry.getValue();
            }
        }
        for (Map.Entry<Long, Node> entry : ring.headMap(hash, false).entrySet()) {
            if (entry.getValue().state() == State.ONLINE) {
                return entry.getValue();
            }
        }
        return null;
    }

    private static String virtualNodeKey(Node node, int index) {
        return node.socketAddress().toString() + "#VN" + index;
    }

    private static long hash(String key) {
        return HASH_FUNCTION.hashString(key, StandardCharsets.UTF_8).asLong();
    }
}
