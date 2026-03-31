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
package com.shieldblaze.expressgateway.protocol.quic;

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import io.netty.handler.codec.quic.QuicChannel;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;

/**
 * Connection pool for QUIC backend connections, supporting stream-counted multiplexing.
 *
 * <p>This pool follows shared-connection semantics: connections are never "checked out",
 * and the pool scans for a connection with available stream capacity. If all connections
 * are at capacity, a new QUIC connection is created up to {@code maxConnectionsPerNode}.</p>
 *
 * <h3>Pool Semantics (shared, stream-counted)</h3>
 * QUIC connections are multiplexed -- many streams share one connection. Each
 * QUIC stream is independent (no head-of-line blocking per RFC 9000 Section 2.1).
 * The pool scans for the connection with the fewest active streams (least-loaded)
 * to evenly distribute request load across connections.
 *
 * <h3>Idle Eviction</h3>
 * QUIC connections become idle when their active stream count drops to zero. A periodic
 * sweep evicts connections that have been idle longer than the configured timeout.
 *
 * @param <C> the connection type, must extend {@link QuicConnection}
 */
public class QuicConnectionPool<C extends QuicConnection> {

    private static final Logger logger = LogManager.getLogger(QuicConnectionPool.class);

    private final ConcurrentHashMap<Node, CopyOnWriteArrayList<C>> pool = new ConcurrentHashMap<>();

    private final int maxPerNode;
    private final int maxConcurrentStreams;
    private final long poolIdleTimeoutNanos;

    private static final long EVICTION_INTERVAL_SECONDS = 30;

    // PERF-3: Shared static executor for all QuicConnectionPool and QuicCidSessionMap instances.
    // Previously each pool created its own ScheduledExecutorService with a daemon thread,
    // causing thread proliferation when multiple QUIC pools are created (e.g., per listener).
    // Matches the pattern used by ConnectionPool.SHARED_EVICTION_EXECUTOR.
    static final ScheduledExecutorService SHARED_EVICTION_EXECUTOR =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "quic-pool-idle-evictor-shared");
                t.setDaemon(true);
                return t;
            });

    private final ScheduledFuture<?> evictionTask;

    public QuicConnectionPool(QuicConfiguration config) {
        this.maxPerNode = config.maxConnectionsPerNode();
        // Clamp to int range to prevent overflow — QUIC uses variable-length ints up to 2^62
        this.maxConcurrentStreams = (int) Math.min(config.initialMaxStreamsBidi(), Integer.MAX_VALUE);
        this.poolIdleTimeoutNanos = TimeUnit.MILLISECONDS.toNanos(config.maxIdleTimeoutMs());

        if (poolIdleTimeoutNanos > 0) {
            evictionTask = SHARED_EVICTION_EXECUTOR.scheduleAtFixedRate(
                    this::evictIdleConnections,
                    EVICTION_INTERVAL_SECONDS,
                    EVICTION_INTERVAL_SECONDS,
                    TimeUnit.SECONDS
            );
        } else {
            evictionTask = null;
        }
    }

    /**
     * Acquire a QUIC backend connection with available stream capacity for the given Node
     * and atomically claim a stream slot. Returns {@code null} if no connection has capacity.
     *
     * <p>Selects the connection with the fewest active streams (least-loaded) and
     * atomically increments the stream count to prevent TOCTOU races where multiple
     * threads observe capacity and all exceed the MAX_STREAMS limit (RFC 9000 Section 4.6).</p>
     *
     * <p>The caller does NOT need to call {@code incrementActiveStreams()} after acquire —
     * the stream slot is already claimed. The caller MUST call {@code decrementActiveStreams()}
     * when the stream completes.</p>
     */
    public C acquire(Node node) {
        CopyOnWriteArrayList<C> conns = pool.get(node);
        if (conns == null) {
            return null;
        }

        C best = null;
        int bestCount = Integer.MAX_VALUE;
        for (C conn : conns) {
            Connection.State state = conn.state();
            if (state == Connection.State.CONNECTION_CLOSED
                    || state == Connection.State.CONNECTION_TIMEOUT) {
                continue;
            }

            QuicChannel qc = conn.quicChannel();
            boolean isActive = qc != null && qc.isActive();
            boolean isInitializing = state == Connection.State.INITIALIZED;

            if ((isActive || isInitializing) && conn.hasStreamCapacity(maxConcurrentStreams)) {
                int count = conn.activeStreams();
                if (count < bestCount) {
                    best = conn;
                    bestCount = count;
                }
            }
        }
        if (best != null) {
            // Atomically claim a stream slot. If we overshoot due to a race,
            // revert and report no capacity.
            int claimed = best.incrementActiveStreams();
            if (claimed > maxConcurrentStreams) {
                best.decrementActiveStreams();
                return null;
            }
            StandardEdgeNetworkMetricRecorder.INSTANCE.recordPoolHit();
        }
        return best;
    }

    /**
     * Returns {@code true} if a new connection can be created for the given Node.
     */
    public boolean canCreateConnection(Node node) {
        CopyOnWriteArrayList<C> conns = pool.get(node);
        return conns == null || conns.size() < maxPerNode;
    }

    /**
     * Atomically register a connection if under the per-node limit.
     * Prevents TOCTOU race between canCreateConnection() and register()
     * where concurrent threads could all pass the check and exceed maxPerNode.
     *
     * @return {@code true} if registered, {@code false} if at capacity
     */
    public boolean tryRegister(Node node, C conn) {
        CopyOnWriteArrayList<C> conns = pool.computeIfAbsent(node, k -> new CopyOnWriteArrayList<>());
        synchronized (conns) {
            if (conns.size() >= maxPerNode) {
                return false;
            }
            conns.add(conn);
            return true;
        }
    }

    /**
     * Register a newly created connection into the pool unconditionally.
     */
    public void register(Node node, C conn) {
        pool.computeIfAbsent(node, k -> new CopyOnWriteArrayList<>()).add(conn);
    }

    /**
     * Signal that a stream on a connection has completed.
     * The caller must have already called {@code conn.decrementActiveStreams()}.
     *
     * @param node the node that owns the connection (reserved for future use)
     * @param conn the connection whose stream completed (reserved for future use)
     */
    @SuppressWarnings("unused")
    public void releaseStream(Node node, C conn) {
        // Connections stay in the pool -- they're shared. Nothing to do here
        // beyond what the caller already did (decrementActiveStreams).
    }

    /**
     * Evict a dead or drained connection from the pool.
     */
    public void evict(C conn) {
        Node node = conn.node();
        if (node == null) return;

        CopyOnWriteArrayList<C> list = pool.get(node);
        if (list != null) {
            list.remove(conn);
        }
    }

    /**
     * Iterate all active connections without allocating a collection.
     */
    public void forEachActiveConnection(Consumer<C> action) {
        for (CopyOnWriteArrayList<C> list : pool.values()) {
            list.forEach(action);
        }
    }

    /**
     * Check whether all active backend QUIC connections are writable.
     */
    public boolean allBackendsWritable() {
        for (CopyOnWriteArrayList<C> list : pool.values()) {
            for (C conn : list) {
                QuicChannel qc = conn.quicChannel();
                if (qc != null && qc.isActive() && !qc.isWritable()) {
                    return false;
                }
            }
        }
        return true;
    }

    /**
     * Returns all active connections across all nodes.
     */
    public Collection<C> allActiveConnections() {
        List<C> all = new ArrayList<>();
        for (CopyOnWriteArrayList<C> list : pool.values()) {
            all.addAll(list);
        }
        return all;
    }

    /**
     * Returns the max concurrent streams per connection.
     */
    public int maxConcurrentStreams() {
        return maxConcurrentStreams;
    }

    /**
     * Periodic sweep that evicts connections idle longer than the configured timeout.
     */
    private void evictIdleConnections() {
        final long now = System.nanoTime();
        int evicted = 0;

        for (Map.Entry<Node, CopyOnWriteArrayList<C>> entry : pool.entrySet()) {
            List<C> toEvict = null;

            for (C conn : entry.getValue()) {
                QuicChannel qc = conn.quicChannel();

                // Evict dead connections opportunistically
                if (qc == null || !qc.isActive()) {
                    if (toEvict == null) toEvict = new ArrayList<>(4);
                    toEvict.add(conn);
                    evicted++;
                    continue;
                }

                if (conn.activeStreams() > 0) {
                    continue;
                }

                long idleSince = conn.idleSinceNanos();
                if (idleSince == 0) {
                    continue;
                }

                if (now - idleSince > poolIdleTimeoutNanos) {
                    // Double-check activeStreams after reading idleSinceNanos to guard
                    // against a race where incrementActiveStreams() clears idleSinceNanos
                    // but we read the stale value before it was cleared.
                    if (conn.activeStreams() > 0) {
                        continue;
                    }
                    if (toEvict == null) toEvict = new ArrayList<>(4);
                    toEvict.add(conn);
                    evicted++;
                }
            }

            if (toEvict != null) {
                CopyOnWriteArrayList<C> list = entry.getValue();
                for (C conn : toEvict) {
                    list.remove(conn);
                    conn.close();
                }
            }
        }

        if (evicted > 0) {
            logger.debug("Evicted {} idle QUIC connection(s) from pool", evicted);
        }
    }

    /**
     * Close all connections in the pool and cancel the eviction task.
     */
    public void closeAll() {
        if (evictionTask != null) {
            evictionTask.cancel(false);
        }
        // PERF-3: Do NOT shut down the shared executor -- it is static and shared
        // across all QuicConnectionPool instances.

        for (CopyOnWriteArrayList<C> list : pool.values()) {
            for (C conn : list) {
                conn.close();
            }
            list.clear();
        }
        pool.clear();
    }
}
