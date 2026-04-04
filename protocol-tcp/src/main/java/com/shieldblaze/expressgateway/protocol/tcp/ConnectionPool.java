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
package com.shieldblaze.expressgateway.protocol.tcp;

import io.netty.channel.Channel;
import lombok.extern.log4j.Log4j2;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Lock-free TCP connection pool for backend connections.
 *
 * <p>Pools idle backend channels keyed by their remote address. When a new proxy
 * session needs a backend connection, it first checks the pool before creating
 * a new one via the bootstrapper.</p>
 *
 * <p>Design decisions:
 * <ul>
 *   <li>Uses {@link ConcurrentLinkedQueue} per backend address for O(1) non-blocking
 *       acquire/release (FIFO order ensures oldest connections are reused first,
 *       which helps detect stale connections quickly).</li>
 *   <li>Each entry is validated via {@link ConnectionPoolEntry#isUsable()} before
 *       returning. Dead connections are silently discarded.</li>
 *   <li>{@link #evictIdle()} runs periodically (caller's responsibility to schedule)
 *       and closes connections idle longer than the configured timeout.</li>
 * </ul></p>
 *
 * <p>Thread safety: all operations are lock-free. ConcurrentHashMap provides per-bin
 * locking for the address map; ConcurrentLinkedQueue provides non-blocking FIFO.</p>
 */
@Log4j2
final class ConnectionPool implements AutoCloseable {

    /**
     * Pool configuration.
     *
     * @param maxIdlePerBackend   maximum idle connections per backend address
     * @param maxTotalConnections maximum total pooled connections across all backends
     * @param idleTimeout         connections idle longer than this are evicted
     * @param warmupCount         number of connections to pre-create per backend on warmup
     */
    record Config(int maxIdlePerBackend, int maxTotalConnections, Duration idleTimeout, int warmupCount) {

        static final Config DEFAULT = new Config(8, 256, Duration.ofSeconds(60), 0);

        Config {
            if (maxIdlePerBackend < 0) throw new IllegalArgumentException("maxIdlePerBackend must be >= 0");
            if (maxTotalConnections < 0) throw new IllegalArgumentException("maxTotalConnections must be >= 0");
            if (idleTimeout.isNegative()) throw new IllegalArgumentException("idleTimeout must be non-negative");
            if (warmupCount < 0) throw new IllegalArgumentException("warmupCount must be >= 0");
        }
    }

    private final ConcurrentHashMap<InetSocketAddress, ConcurrentLinkedQueue<ConnectionPoolEntry>> pool;
    private final Config config;
    private final AtomicInteger totalSize;
    private final AtomicBoolean closed;

    ConnectionPool(Config config) {
        this.config = config;
        this.pool = new ConcurrentHashMap<>();
        this.totalSize = new AtomicInteger(0);
        this.closed = new AtomicBoolean(false);
    }

    ConnectionPool() {
        this(Config.DEFAULT);
    }

    /**
     * Try to acquire an idle connection to the given backend address.
     *
     * @param backendAddress the backend address to look up
     * @return a usable channel, or {@code null} if no pooled connection is available
     */
    Channel acquire(InetSocketAddress backendAddress) {
        if (closed.get()) {
            return null;
        }

        ConcurrentLinkedQueue<ConnectionPoolEntry> queue = pool.get(backendAddress);
        if (queue == null) {
            return null;
        }

        ConnectionPoolEntry entry;
        while ((entry = queue.poll()) != null) {
            totalSize.decrementAndGet();
            if (entry.isUsable()) {
                log.debug("Pool hit for backend {}", backendAddress);
                return entry.channel();
            }
            // Dead connection -- close and try next
            entry.channel().close();
        }

        return null;
    }

    /**
     * Release a backend connection back to the pool.
     *
     * <p>The connection is only pooled if:
     * <ul>
     *   <li>The pool is not closed</li>
     *   <li>The channel is still active</li>
     *   <li>The per-backend idle limit is not exceeded</li>
     *   <li>The total pool size limit is not exceeded</li>
     * </ul>
     * Otherwise the channel is closed immediately.</p>
     *
     * @param channel        the backend channel to return
     * @param backendAddress the backend address this channel connects to
     * @return true if the connection was pooled, false if it was closed
     */
    boolean release(Channel channel, InetSocketAddress backendAddress) {
        if (closed.get() || !channel.isActive()) {
            channel.close();
            return false;
        }

        ConcurrentLinkedQueue<ConnectionPoolEntry> queue =
                pool.computeIfAbsent(backendAddress, k -> new ConcurrentLinkedQueue<>());

        // Check per-backend limit (approximate -- ConcurrentLinkedQueue.size() is O(n)
        // but acceptable here since release is not a hot-path operation compared to reads)
        if (queue.size() >= config.maxIdlePerBackend()) {
            channel.close();
            return false;
        }

        // Check total limit
        if (totalSize.get() >= config.maxTotalConnections()) {
            channel.close();
            return false;
        }

        // Reset autoRead before pooling. Backpressure handling may have toggled
        // autoRead to false during the previous proxy session. A pooled connection
        // with autoRead=false would silently stop reading when reused, causing
        // the downstream to appear hung.
        if (channel.config() != null) {
            channel.config().setAutoRead(true);
        }

        queue.offer(new ConnectionPoolEntry(channel, backendAddress));
        totalSize.incrementAndGet();
        log.debug("Released connection to pool for backend {} (pool size: {})",
                backendAddress, totalSize.get());
        return true;
    }

    /**
     * Evict connections that have been idle longer than {@link Config#idleTimeout()}.
     * This method should be called periodically by a scheduled task.
     *
     * <p>Uses a drain-and-re-add approach: all entries are polled from the queue,
     * survivors are collected, then re-offered back. This is O(n) total vs O(n^2)
     * for Iterator.remove() on ConcurrentLinkedQueue (which re-traverses from head).
     * Entries added by concurrent {@link #release} calls during the drain are safe:
     * they remain in the queue because poll() only removes from the head.</p>
     *
     * @return number of connections evicted
     */
    int evictIdle() {
        int evicted = 0;
        for (var mapEntry : pool.entrySet()) {
            ConcurrentLinkedQueue<ConnectionPoolEntry> queue = mapEntry.getValue();
            int batchSize = queue.size();
            int evictedFromQueue = 0;

            // Drain at most batchSize entries to avoid spinning on concurrent additions.
            // Any entries added by release() after we start are left in the queue untouched.
            for (int i = 0; i < batchSize; i++) {
                ConnectionPoolEntry poolEntry = queue.poll();
                if (poolEntry == null) {
                    break;
                }
                if (poolEntry.isUsable() && !poolEntry.isIdleLongerThan(config.idleTimeout())) {
                    queue.offer(poolEntry);
                } else {
                    totalSize.decrementAndGet();
                    poolEntry.channel().close();
                    evictedFromQueue++;
                }
            }
            evicted += evictedFromQueue;

            // Clean up empty queues to prevent map growth
            if (queue.isEmpty()) {
                pool.remove(mapEntry.getKey(), queue);
            }
        }
        if (evicted > 0) {
            log.debug("Evicted {} idle connections from pool (remaining: {})", evicted, totalSize.get());
        }
        return evicted;
    }

    /**
     * Current number of pooled connections.
     */
    int size() {
        return totalSize.get();
    }

    /**
     * Number of pooled connections for a specific backend.
     */
    int size(InetSocketAddress backendAddress) {
        ConcurrentLinkedQueue<ConnectionPoolEntry> queue = pool.get(backendAddress);
        return queue == null ? 0 : queue.size();
    }

    /**
     * Pool configuration.
     */
    Config config() {
        return config;
    }

    @Override
    public void close() {
        if (closed.compareAndSet(false, true)) {
            for (ConcurrentLinkedQueue<ConnectionPoolEntry> queue : pool.values()) {
                ConnectionPoolEntry entry;
                while ((entry = queue.poll()) != null) {
                    totalSize.decrementAndGet();
                    entry.channel().close();
                }
            }
            pool.clear();
            log.info("Connection pool closed");
        }
    }
}
