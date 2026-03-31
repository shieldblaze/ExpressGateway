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
import io.netty.util.NetUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.math.BigInteger;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Iterator;
import java.util.concurrent.ConcurrentHashMap;

public final class SourceIPHash implements SessionPersistence<Node, Node, InetSocketAddress, Node> {

    private static final Logger logger = LogManager.getLogger(SourceIPHash.class);

    private static final BigInteger MINUS_ONE = BigInteger.valueOf(-1);

    /**
     * LB-F3: Maximum number of session persistence entries.
     * <p>
     * At ~64 bytes per entry (masked IP key + Node reference + timestamp), 100_000
     * entries consumes roughly 10-15 MB. The /24 (IPv4) and /48 (IPv6) masking means
     * one entry per subnet, so 100K entries covers 100K distinct subnets which is
     * generous for most deployments.
     */
    static final int DEFAULT_MAX_ENTRIES = 100_000;

    private final SelfExpiringMap<Object, Node> routeMap = new SelfExpiringMap<>(new ConcurrentHashMap<>(), Duration.ofHours(1), false);
    private final int maxEntries;

    /**
     * Create a {@link SourceIPHash} instance with the default max entries limit.
     */
    public SourceIPHash() {
        this(DEFAULT_MAX_ENTRIES);
    }

    /**
     * Create a {@link SourceIPHash} instance with a custom max entries limit.
     *
     * @param maxEntries Maximum number of session persistence entries before eviction.
     *                   Must be positive.
     */
    public SourceIPHash(int maxEntries) {
        if (maxEntries <= 0) {
            throw new IllegalArgumentException("maxEntries must be positive: " + maxEntries);
        }
        this.maxEntries = maxEntries;
    }

    @Override
    public Node node(Request request) {
        Node node;

        /*
         * If Source IP Address is IPv4, we'll convert it into Integer with /24 mask.
         *
         * If Source IP Address is IPv6, we'll convert it into BigInteger with /48 mask.
         */
        if (request.socketAddress().getAddress() instanceof Inet4Address) {
            int ipWithMask = ipv4WithMask(request);
            node = routeMap.get(ipWithMask);
        } else {
            BigInteger ipWithMask = ipv6WithMask(request);
            node = routeMap.get(ipWithMask);
        }

        return node;
    }

    @Override
    public Node addRoute(InetSocketAddress socketAddress, Node node) {
        Object key;
        if (socketAddress.getAddress() instanceof Inet4Address) {
            key = ipv4WithMask(socketAddress);
        } else {
            key = ipv6WithMask(socketAddress);
        }

        // LB-F3: Enforce max size to prevent unbounded memory growth.
        // When the map is at capacity and this is a new key, evict a batch of the
        // oldest entries. ConcurrentHashMap iteration order is arbitrary but stable
        // enough for eviction purposes -- we just need to shed load, not guarantee
        // perfect LRU ordering. Evicting ~10% amortizes the cost of iteration.
        if (routeMap.size() >= maxEntries && !routeMap.containsKey(key)) {
            evictOldest();
        }

        routeMap.put(key, node);
        return node;
    }

    @Override
    public boolean removeRoute(InetSocketAddress socketAddress, Node node) {
        Object key;
        if (socketAddress.getAddress() instanceof Inet4Address) {
            key = ipv4WithMask(socketAddress);
        } else {
            key = ipv6WithMask(socketAddress);
        }
        return routeMap.remove(key, node);
    }

    @Override
    public boolean remove(Node nodeToRemove) {
        return routeMap.entrySet().removeIf(entry -> entry.getValue() == nodeToRemove);
    }

    @Override
    public void clear() {
        routeMap.clear();
    }

    @Override
    public String toString() {
        return "SourceIPHash{" +
                "routeMap=" + routeMap +
                '}';
    }

    @Override
    public String name() {
        return "SourceIPHash";
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
     * memory -- not to preserve the most valuable sessions. Expired entries are naturally
     * cleaned up by the SelfExpiringMap's background cleaner.
     */
    private void evictOldest() {
        int toEvict = Math.max(1, maxEntries / 10);
        int evicted = 0;
        Iterator<java.util.Map.Entry<Object, Node>> it = routeMap.entrySet().iterator();
        while (it.hasNext() && evicted < toEvict) {
            it.next();
            it.remove();
            evicted++;
        }
        if (evicted > 0) {
            logger.debug("SourceIPHash: evicted {} entries (map size was at capacity {})", evicted, maxEntries);
        }
    }

    private static int ipv4WithMask(Request request) {
        return ipv4WithMask(request.socketAddress());
    }

    private static BigInteger ipv6WithMask(Request request) {
        return ipv6WithMask(request.socketAddress());
    }

    private static int ipv4WithMask(InetSocketAddress socketAddress) {
        int ipAddress = NetUtil.ipv4AddressToInt((Inet4Address) socketAddress.getAddress());
        return ipAddress & prefixToSubnetMaskIPv4();
    }

    private static BigInteger ipv6WithMask(InetSocketAddress socketAddress) {
        BigInteger ipAddress = ipToInt((Inet6Address) socketAddress.getAddress());
        return ipAddress.and(prefixToSubnetMaskIPv6());
    }

    private static BigInteger ipToInt(Inet6Address ipAddress) {
        return new BigInteger(1, ipAddress.getAddress());
    }

    private static int prefixToSubnetMaskIPv4() {
        return (int) (-1L << 32 - 24);
    }

    private static BigInteger prefixToSubnetMaskIPv6() {
        return MINUS_ONE.shiftLeft(128 - 48);
    }
}
