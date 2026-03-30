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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.common.map.SelfExpiringMap;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Iterator;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * <p> 4-Tuple Hash based {@link SessionPersistence} </p>
 * <p> Source IP Address + Source Port + Destination IP Address + Destination Port </p>
 */
public final class FourTupleHash implements SessionPersistence<Node, Node, InetSocketAddress, Node> {

    private static final Logger logger = LogManager.getLogger(FourTupleHash.class);

    /**
     * LB-F3: Maximum number of session persistence entries.
     * <p>
     * The 4-tuple hash has one entry per unique (srcIP, srcPort, dstIP, dstPort)
     * combination. Under high connection churn, this can grow very large.
     * Each entry is ~80 bytes (InetSocketAddress key + Node reference + timestamp),
     * so 500K entries consumes roughly 50-60 MB.
     */
    static final int DEFAULT_MAX_ENTRIES = 500_000;

    private final Map<InetSocketAddress, Node> routeMap = new SelfExpiringMap<>(new ConcurrentHashMap<>(), Duration.ofHours(1), false);
    private final int maxEntries;

    /**
     * Create a {@link FourTupleHash} instance with the default max entries limit.
     */
    public FourTupleHash() {
        this(DEFAULT_MAX_ENTRIES);
    }

    /**
     * Create a {@link FourTupleHash} instance with a custom max entries limit.
     *
     * @param maxEntries Maximum number of session persistence entries before eviction.
     *                   Must be positive.
     */
    public FourTupleHash(int maxEntries) {
        if (maxEntries <= 0) {
            throw new IllegalArgumentException("maxEntries must be positive: " + maxEntries);
        }
        this.maxEntries = maxEntries;
    }

    @Override
    public Node node(Request request) {
        return routeMap.get(request.socketAddress());
    }

    @Override
    public Node addRoute(InetSocketAddress socketAddress, Node node) {
        // LB-F3: Enforce max size to prevent unbounded memory growth.
        // When the map is at capacity and this is a new key, evict a batch of
        // entries. Evicting ~10% amortizes the cost of iteration.
        if (routeMap.size() >= maxEntries && !routeMap.containsKey(socketAddress)) {
            evictOldest();
        }
        routeMap.put(socketAddress, node);
        return node;
    }

    @Override
    public boolean removeRoute(InetSocketAddress inetSocketAddress, Node node) {
        return routeMap.remove(inetSocketAddress, node);
    }

    @Override
    public boolean remove(Node nodeToRemove) {
        return routeMap.entrySet().removeIf(entry -> entry.getValue() == nodeToRemove);
    }

    @Override
    public void clear() {
        routeMap.clear();
    }

    /**
     * Returns the current number of entries in the session map.
     * Useful for monitoring and metrics.
     */
    public int size() {
        return routeMap.size();
    }

    /**
     * LB-F3: Evicts approximately 10% of entries to make room for new sessions.
     * <p>
     * We evict a batch rather than a single entry to amortize the O(n) iteration cost
     * over multiple subsequent puts. The ConcurrentHashMap iterator does not guarantee
     * oldest-first ordering, but for session persistence the goal is simply to bound
     * memory -- not to preserve the most valuable sessions.
     */
    private void evictOldest() {
        int toEvict = Math.max(1, maxEntries / 10);
        int evicted = 0;
        Iterator<Map.Entry<InetSocketAddress, Node>> it = routeMap.entrySet().iterator();
        while (it.hasNext() && evicted < toEvict) {
            it.next();
            it.remove();
            evicted++;
        }
        if (evicted > 0) {
            logger.debug("FourTupleHash: evicted {} entries (map size was at capacity {})", evicted, maxEntries);
        }
    }

    @Override
    public String toString() {
        return "FourTupleHash{" +
                "routeMap=" + routeMap +
                '}';
    }

    @Override
    public String name() {
        return "FourTupleHash";
    }
}
