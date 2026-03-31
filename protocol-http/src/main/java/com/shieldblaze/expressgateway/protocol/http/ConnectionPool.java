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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.MemoryBudget;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import io.netty.util.AttributeKey;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Deque;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedDeque;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Connection pool for backend HTTP connections, supporting both HTTP/1.1 (serial,
 * checkout/return) and HTTP/2 (shared, stream-counted) connection models.
 *
 * <p>This pool enables per-request/per-stream load balancing, matching the behavior
 * of Nginx, Envoy, and HAProxy. Each request or H2 stream can be independently
 * routed to any backend Node, and connections are reused from the pool to avoid
 * the overhead of establishing new TCP+TLS connections per request.</p>
 *
 * <h3>H1 Pool Semantics (checkout/return)</h3>
 * H1 connections are serial — one request at a time. When acquired, the connection
 * is removed from the idle pool and marked in-use. When the response completes,
 * it's returned to the pool for reuse by the next request. LIFO ordering reuses
 * warm connections (TCP windows already open, TLS sessions cached).
 *
 * <h3>H2 Pool Semantics (shared, stream-counted)</h3>
 * H2 connections are multiplexed — many streams share one connection. They are
 * never "checked out." The pool scans for a connection with available stream
 * capacity ({@code activeStreams < maxConcurrentStreams}). If all are at capacity,
 * a new connection is created up to {@code maxH2ConnectionsPerNode}.
 *
 * <h3>Thread Safety</h3>
 * The pool is accessed from the frontend EventLoop (acquire), the backend EventLoop
 * (release on response completion), and close listeners (eviction). All structures
 * use concurrent collections.
 */
public final class ConnectionPool {

    private static final Logger logger = LogManager.getLogger(ConnectionPool.class);

    /**
     * F-16: Attribute key to ensure the H1 close listener is registered exactly once
     * per channel, preventing listener accumulation across acquire/release cycles.
     */
    private static final AttributeKey<Boolean> H1_CLOSE_LISTENER_KEY =
            AttributeKey.valueOf("h1CloseListenerRegistered");

    /**
     * Idle H1 connections per Node. LIFO via pollFirst/addFirst for warm reuse.
     */
    private final ConcurrentHashMap<Node, Deque<HttpConnection>> h1Pool = new ConcurrentHashMap<>();

    /**
     * NP-03: Atomic counter for H1 pool size per Node. ConcurrentLinkedDeque.size() is O(n);
     * this counter provides O(1) size checks in releaseH1().
     */
    private final ConcurrentHashMap<Node, java.util.concurrent.atomic.AtomicInteger> h1PoolSize = new ConcurrentHashMap<>();

    /**
     * Active H2 connections per Node. Shared — multiple streams use each connection.
     * CopyOnWriteArrayList because adds/removes (connection lifecycle) are rare vs
     * reads (every stream routing).
     */
    private final ConcurrentHashMap<Node, CopyOnWriteArrayList<HttpConnection>> h2Pool = new ConcurrentHashMap<>();

    private final int maxH1PerNode;
    private final int maxH2PerNode;
    private final int maxConcurrentStreams;

    /**
     * RES-02: Idle timeout for pooled H1 connections, in nanoseconds.
     * Connections idle longer than this are proactively evicted by the sweep task.
     * A value of 0 disables idle eviction entirely.
     */
    private final long poolIdleTimeoutNanos;

    /**
     * MEM-01: Maximum connection age in nanoseconds. Connections older than this
     * are evicted during the periodic sweep regardless of idle status. Prevents
     * stale TCP connections that may have degraded backend-side state (e.g.,
     * server-side connection tracking limits, in-kernel buffer bloat, DNS changes).
     * A value of 0 disables max-age eviction.
     */
    private final long maxConnectionAgeNanos;

    /**
     * RES-02: Hardcoded eviction sweep interval (30 seconds).
     */
    private static final long EVICTION_INTERVAL_SECONDS = 30;

    /**
     * PERF-POOL-01 FIX: Shared static executor for all ConnectionPool instances.
     * Previously each ConnectionPool created its own ScheduledExecutorService,
     * causing thread exhaustion at high connection counts (10K clients = 10K threads).
     * A single shared daemon thread handles all eviction sweeps.
     */
    private static final ScheduledExecutorService SHARED_EVICTION_EXECUTOR =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "pool-idle-evictor-shared");
                t.setDaemon(true);
                return t;
            });

    /**
     * RES-02: Handle to the scheduled eviction task for cancellation in closeAll().
     */
    private final ScheduledFuture<?> evictionTask;

    /**
     * STAT-01: Connection pool statistics counters for observability.
     */
    private final ConnectionPoolStats stats = new ConnectionPoolStats();

    /**
     * DEF-CP-01: Tracks the number of H1 connections currently checked out (in-use).
     * Incremented on acquireH1(), decremented on releaseH1() and when a checked-out
     * connection's channel closes. Provides accurate reporting via getActiveH1Connections().
     */
    private final java.util.concurrent.atomic.AtomicInteger activeH1Count = new java.util.concurrent.atomic.AtomicInteger();

    /**
     * MEM-03: Optional global memory budget. When non-null, the pool uses it for
     * admission control during backlog growth. Null means no memory budget enforcement.
     */
    private volatile MemoryBudget memoryBudget;

    public ConnectionPool(HttpConfiguration config) {
        this(config, Duration.ofMinutes(5));
    }

    /**
     * Create a ConnectionPool with configurable max connection age.
     *
     * @param config          HTTP configuration
     * @param maxConnectionAge maximum age for connections before eviction; {@link Duration#ZERO} to disable
     */
    public ConnectionPool(HttpConfiguration config, Duration maxConnectionAge) {
        this.maxH1PerNode = config.maxH1ConnectionsPerNode();
        this.maxH2PerNode = config.maxH2ConnectionsPerNode();
        this.maxConcurrentStreams = (int) config.maxConcurrentStreams();
        this.poolIdleTimeoutNanos = TimeUnit.SECONDS.toNanos(config.poolIdleTimeoutSeconds());
        this.maxConnectionAgeNanos = maxConnectionAge.toNanos();

        boolean needsSweep = poolIdleTimeoutNanos > 0 || maxConnectionAgeNanos > 0;
        if (needsSweep) {
            evictionTask = SHARED_EVICTION_EXECUTOR.scheduleAtFixedRate(
                    this::evictConnections,
                    EVICTION_INTERVAL_SECONDS,
                    EVICTION_INTERVAL_SECONDS,
                    TimeUnit.SECONDS
            );
        } else {
            evictionTask = null;
        }
    }

    /**
     * MEM-03: Set the global memory budget for admission control.
     * When set, the pool can integrate with backlog shedding decisions.
     *
     * @param memoryBudget the memory budget to use, or {@code null} to disable
     */
    public void setMemoryBudget(MemoryBudget memoryBudget) {
        this.memoryBudget = memoryBudget;
    }

    /**
     * MEM-03: Returns the current memory budget, or {@code null} if not set.
     */
    public MemoryBudget getMemoryBudget() {
        return memoryBudget;
    }

    /**
     * MEM-03: Returns {@code true} if the memory budget is set and current usage
     * exceeds the given threshold. Useful for load shedding decisions.
     *
     * @param threshold ratio threshold (e.g., 0.9 for 90%)
     * @return {@code true} if over the memory threshold, {@code false} if no budget
     *         is set or usage is within budget
     */
    public boolean isMemoryPressured(double threshold) {
        MemoryBudget budget = this.memoryBudget;
        return budget != null && budget.isOverThreshold(threshold);
    }

    /**
     * Acquire an idle H1 backend connection for the given Node.
     * Returns {@code null} if no idle connection is available (caller must create a new one).
     */
    public HttpConnection acquireH1(Node node) {
        Deque<HttpConnection> deque = h1Pool.get(node);
        if (deque == null) {
            return null;
        }

        java.util.concurrent.atomic.AtomicInteger sizeCounter = h1PoolSize.get(node);
        HttpConnection conn;
        while ((conn = deque.pollFirst()) != null) {
            if (sizeCounter != null) sizeCounter.decrementAndGet();
            // Validate the connection is still alive
            if (conn.channel() != null && conn.channel().isActive()) {
                // MEM-01: Skip connections that have exceeded max age
                if (maxConnectionAgeNanos > 0 && isExpired(conn)) {
                    stats.totalEvictions.incrementAndGet();
                    conn.close();
                    logger.debug("MEM-01: Skipping max-age-expired H1 connection to {}", node.socketAddress());
                    continue;
                }
                conn.markInUse();
                activeH1Count.incrementAndGet();
                StandardEdgeNetworkMetricRecorder.INSTANCE.recordPoolHit();
                return conn;
            }
            // Dead connection — discard
            logger.debug("Discarding dead H1 pooled connection to {}", node.socketAddress());
        }
        return null;
    }

    /**
     * Acquire an H2 backend connection with available stream capacity for the given Node.
     * Returns {@code null} if no connection has capacity and the max connections limit
     * has been reached (caller should create a new one if under limit).
     *
     * <p>Connections that are still initializing (ALPN pending, channel not yet active)
     * are also eligible — writes will be queued in the connection's backlog and flushed
     * when ALPN completes. This prevents creating duplicate connections for concurrent
     * streams that arrive before the first connection's ALPN handshake finishes.</p>
     */
    HttpConnection acquireH2(Node node) {
        CopyOnWriteArrayList<HttpConnection> conns = h2Pool.get(node);
        if (conns == null) {
            return null;
        }

        // CM-D1 FIX: Two-phase acquire — first find the least-loaded candidate,
        // then atomically claim a stream slot via CAS. If the CAS fails (another
        // thread claimed the last slot), retry with the next-best candidate.
        // This eliminates the TOCTOU race where hasStreamCapacity() returns true
        // but the slot is taken before incrementActiveStreams().
        for (;;) {
            HttpConnection best = null;
            int bestCount = Integer.MAX_VALUE;
            for (HttpConnection conn : conns) {
                // Skip dead connections
                com.shieldblaze.expressgateway.backend.Connection.State state = conn.state();
                if (state == com.shieldblaze.expressgateway.backend.Connection.State.CONNECTION_CLOSED
                        || state == com.shieldblaze.expressgateway.backend.Connection.State.CONNECTION_TIMEOUT) {
                    continue;
                }

                // Accept connections that are still connecting (INITIALIZED) or active
                boolean isActive = conn.channel() != null && conn.channel().isActive();
                boolean isInitializing = state == com.shieldblaze.expressgateway.backend.Connection.State.INITIALIZED;

                if ((isActive || isInitializing) && conn.hasStreamCapacity(maxConcurrentStreams)) {
                    int count = conn.activeStreams();
                    if (count < bestCount) {
                        best = conn;
                        bestCount = count;
                    }
                }
            }

            if (best == null) {
                return null;
            }

            // Atomically claim a stream slot — if this fails, another thread took
            // the last slot, so loop back and find the next candidate.
            if (best.tryIncrementActiveStreams(maxConcurrentStreams)) {
                StandardEdgeNetworkMetricRecorder.INSTANCE.recordPoolHit();
                return best;
            }
            // CAS failed — retry scan
        }
    }

    /**
     * Returns {@code true} if a new H2 connection can be created for the given Node
     * (i.e., the current count is below the configured maximum).
     */
    boolean canCreateH2Connection(Node node) {
        CopyOnWriteArrayList<HttpConnection> conns = h2Pool.get(node);
        return conns == null || conns.size() < maxH2PerNode;
    }

    /**
     * Register a newly created connection into the pool.
     * Called after Bootstrapper.create() for connections that should be pooled.
     *
     * <p>The {@code isH2} parameter is required because ALPN negotiation is asynchronous:
     * at registration time, {@code conn.isHttp2()} may still return {@code false} even
     * though the caller knows the connection will be H2 (TLS+ALPN is configured). The
     * caller must pass the expected protocol type explicitly.</p>
     *
     * @param node the backend node this connection targets
     * @param conn the newly created connection
     * @param isH2 {@code true} if this is an H2 connection (caller-determined)
     */
    void register(Node node, HttpConnection conn, boolean isH2) {
        stats.totalConnectionsCreated.incrementAndGet();

        // MEM-02: Wire memory pressure supplier into the connection so that
        // Connection.effectiveBacklogLimit() can reduce backlog limits under pressure.
        // Without this wiring, the backlog shedding feature is non-functional.
        MemoryBudget budget = this.memoryBudget;
        if (budget != null) {
            conn.setMemoryPressureSupplier(() -> budget.isOverThreshold(0.9));
        }

        if (isH2) {
            h2Pool.computeIfAbsent(node, k -> new CopyOnWriteArrayList<>()).add(conn);
        }
        // H1 connections are registered on release (when they become idle),
        // not on creation (when they're immediately in-use).
    }

    /**
     * Return an H1 connection to the idle pool after its response completes.
     * If the pool is full, the connection is closed instead.
     */
    public void releaseH1(Node node, HttpConnection conn) {
        conn.markIdle();
        activeH1Count.decrementAndGet();

        // Don't pool dead connections
        if (conn.channel() == null || !conn.channel().isActive()) {
            return;
        }

        // MEM-01: Don't pool connections that have exceeded max age
        if (maxConnectionAgeNanos > 0 && isExpired(conn)) {
            stats.totalEvictions.incrementAndGet();
            conn.close();
            logger.debug("MEM-01: Not pooling max-age-expired H1 connection to {}", node.socketAddress());
            return;
        }

        Deque<HttpConnection> deque = h1Pool.computeIfAbsent(node, k -> new ConcurrentLinkedDeque<>());
        // NP-03: Use atomic counter instead of O(n) deque.size().
        // CAS loop atomically reserves a slot only if below max, preventing any
        // transient over-count that the increment-then-rollback pattern would allow.
        java.util.concurrent.atomic.AtomicInteger sizeCounter =
                h1PoolSize.computeIfAbsent(node, k -> new java.util.concurrent.atomic.AtomicInteger());
        int current;
        do {
            current = sizeCounter.get();
            if (current >= maxH1PerNode) {
                // Pool full — close excess connection
                stats.totalConnectionsClosed.incrementAndGet();
                conn.close();
                return;
            }
        } while (!sizeCounter.compareAndSet(current, current + 1));

        // Successfully reserved a slot — add to pool
        deque.addFirst(conn); // LIFO: warm connection reuse

        // F-16: Register a close listener for proactive eviction of dead H1 connections.
        // Use channel Attribute to ensure the listener is registered exactly once per
        // channel, even if the connection goes through multiple acquire/release cycles.
        // Without this, a dead H1 connection sits in the pool until the next acquire
        // attempt discards it or the 30s eviction sweep removes it.
        if (conn.channel().attr(H1_CLOSE_LISTENER_KEY).setIfAbsent(Boolean.TRUE) == null) {
            conn.channel().closeFuture().addListener(future -> evict(conn));
        }
    }

    /**
     * Signal that a stream on an H2 connection has completed.
     * Decrements the active stream count (caller must have already called
     * {@code conn.decrementActiveStreams()}).
     *
     * <p>The {@code node} and {@code conn} parameters are intentionally retained for API
     * symmetry with {@link #releaseH1(Node, HttpConnection)} and to support future
     * H2 connection lifecycle management (e.g., closing idle connections with 0 streams).</p>
     */
    @SuppressWarnings("unused") // Parameters retained for API symmetry and future use
    void releaseH2Stream(Node node, HttpConnection conn) {
        // H2 connections stay in the pool — they're shared. Nothing to do here
        // beyond what the caller already did (decrementActiveStreams).
        // If the connection has 0 active streams and is idle, it could be
        // closed for resource savings, but we keep it for reuse.
    }

    /**
     * Evict a dead connection from all pools.
     */
    public void evict(HttpConnection conn) {
        Node node = conn.node();
        if (node == null) return;

        Deque<HttpConnection> h1Deque = h1Pool.get(node);
        if (h1Deque != null && h1Deque.remove(conn)) {
            java.util.concurrent.atomic.AtomicInteger counter = h1PoolSize.get(node);
            if (counter != null) counter.decrementAndGet();
        }

        CopyOnWriteArrayList<HttpConnection> h2List = h2Pool.get(node);
        if (h2List != null) {
            h2List.remove(conn);
        }
    }

    /**
     * WARM-01: Pre-create connections to a backend node to avoid cold-start latency.
     * Creates {@code count} connections asynchronously, respecting pool limits.
     *
     * <p>This method is non-blocking. Connections are created via the provided
     * {@link Bootstrapper} and registered into the pool. For H1, connections are
     * immediately released into the idle pool. For H2, connections are registered
     * and available for stream routing.</p>
     *
     * <p>The actual number of connections created may be less than {@code count}
     * if pool limits would be exceeded.</p>
     *
     * @param node         the backend node to pre-connect to
     * @param count        number of connections to create
     * @param bootstrapper the bootstrapper used to create connections
     * @param clientChannel a channel reference for the bootstrapper (may be the server channel)
     * @param isH2         whether to create H2 connections (true) or H1 (false)
     */
    public void warmup(Node node, int count, Bootstrapper bootstrapper,
                       io.netty.channel.Channel clientChannel, boolean isH2) {
        if (count <= 0 || node == null || bootstrapper == null) {
            return;
        }

        if (isH2) {
            int existing = 0;
            CopyOnWriteArrayList<HttpConnection> conns = h2Pool.get(node);
            if (conns != null) {
                existing = conns.size();
            }
            int toCreate = Math.min(count, maxH2PerNode - existing);
            for (int i = 0; i < toCreate; i++) {
                try {
                    HttpConnection conn = bootstrapper.create(node, clientChannel, this);
                    register(node, conn, true);
                    logger.debug("WARM-01: Pre-created H2 connection {}/{} to {}",
                            i + 1, toCreate, node.socketAddress());
                } catch (Exception e) {
                    logger.warn("WARM-01: Failed to pre-create H2 connection to {}: {}",
                            node.socketAddress(), e.getMessage());
                    break;
                }
            }
        } else {
            java.util.concurrent.atomic.AtomicInteger sizeCounter = h1PoolSize.get(node);
            int existing = sizeCounter != null ? sizeCounter.get() : 0;
            int toCreate = Math.min(count, maxH1PerNode - existing);
            for (int i = 0; i < toCreate; i++) {
                try {
                    HttpConnection conn = bootstrapper.create(node, clientChannel, this);
                    // For H1 warmup, we register the connection's close future to track it,
                    // and release it into the idle pool once connected.
                    final int index = i;
                    conn.channelFuture().addListener(future -> {
                        if (future.isSuccess()) {
                            releaseH1(node, conn);
                            logger.debug("WARM-01: Pre-created H1 connection {}/{} to {}",
                                    index + 1, toCreate, node.socketAddress());
                        }
                    });
                } catch (Exception e) {
                    logger.warn("WARM-01: Failed to pre-create H1 connection to {}: {}",
                            node.socketAddress(), e.getMessage());
                    break;
                }
            }
        }
    }

    /**
     * GC-05: Iterate all active connections without allocating a collection.
     * Used for backpressure propagation (channelWritabilityChanged) which fires
     * frequently — avoiding ArrayList allocation per call.
     */
    void forEachActiveConnection(java.util.function.Consumer<HttpConnection> action) {
        for (Deque<HttpConnection> deque : h1Pool.values()) {
            deque.forEach(action);
        }
        for (CopyOnWriteArrayList<HttpConnection> list : h2Pool.values()) {
            list.forEach(action);
        }
    }

    /**
     * GC-05: Check whether all active backend connections are writable.
     * Returns {@code true} if every active connection's channel reports writable,
     * or if there are no active connections. Used for aggregate backpressure:
     * the frontend should only read when ALL backends can accept writes.
     */
    boolean allBackendsWritable() {
        for (Deque<HttpConnection> deque : h1Pool.values()) {
            for (HttpConnection conn : deque) {
                io.netty.channel.Channel ch = conn.channel();
                if (ch != null && ch.isActive() && !ch.isWritable()) {
                    return false;
                }
            }
        }
        for (CopyOnWriteArrayList<HttpConnection> list : h2Pool.values()) {
            for (HttpConnection conn : list) {
                io.netty.channel.Channel ch = conn.channel();
                if (ch != null && ch.isActive() && !ch.isWritable()) {
                    return false;
                }
            }
        }
        return true;
    }

    /**
     * Returns all active connections across all pools.
     * Allocates a new list — use {@link #forEachActiveConnection} for hot paths.
     */
    Collection<HttpConnection> allActiveConnections() {
        List<HttpConnection> all = new ArrayList<>();
        forEachActiveConnection(all::add);
        return all;
    }

    /**
     * Returns all active H2 connections. Used for GOAWAY forwarding.
     */
    Collection<HttpConnection> allActiveH2Connections() {
        List<HttpConnection> all = new ArrayList<>();
        for (CopyOnWriteArrayList<HttpConnection> list : h2Pool.values()) {
            all.addAll(list);
        }
        return all;
    }

    /**
     * RES-02 / MEM-01: Periodic sweep that evicts connections based on:
     * 1. Dead channels (opportunistic cleanup)
     * 2. Idle timeout (H1 connections idle longer than configured threshold)
     * 3. Max connection age (connections older than maxConnectionAge, regardless of idle state)
     *
     * Also sweeps H2 connections for max-age eviction since H2 connections persist
     * in the pool for the lifetime of the frontend connection.
     *
     * Called from the eviction executor thread.
     */
    private void evictConnections() {
        final long now = System.nanoTime();
        int evicted = 0;

        // --- H1 pool sweep ---
        for (Map.Entry<Node, Deque<HttpConnection>> entry : h1Pool.entrySet()) {
            Iterator<HttpConnection> it = entry.getValue().iterator();
            while (it.hasNext()) {
                HttpConnection conn = it.next();

                // Evict dead connections opportunistically
                if (conn.channel() == null || !conn.channel().isActive()) {
                    // Delegate to evict() for unified counter management.
                    // evict() uses deque.remove() which is safe concurrent with iteration.
                    evict(conn);
                    evicted++;
                    continue;
                }

                // MEM-01: Evict connections that have exceeded max age
                if (maxConnectionAgeNanos > 0 && isExpired(conn, now)) {
                    evict(conn);
                    conn.close();
                    stats.totalEvictions.incrementAndGet();
                    stats.totalConnectionsClosed.incrementAndGet();
                    evicted++;
                    continue;
                }

                // RES-02: Evict idle connections
                if (poolIdleTimeoutNanos > 0) {
                    long idleSince = conn.idleSinceNanos();
                    if (idleSince == 0) {
                        continue;
                    }

                    if (now - idleSince > poolIdleTimeoutNanos) {
                        // Delegate to evict() for unified counter management, then close.
                        evict(conn);
                        conn.close();
                        stats.totalEvictions.incrementAndGet();
                        stats.totalConnectionsClosed.incrementAndGet();
                        evicted++;
                    }
                }
            }
        }

        // --- H2 pool sweep (max-age only) ---
        if (maxConnectionAgeNanos > 0) {
            for (Map.Entry<Node, CopyOnWriteArrayList<HttpConnection>> entry : h2Pool.entrySet()) {
                for (HttpConnection conn : entry.getValue()) {
                    // Evict dead H2 connections
                    if (conn.channel() == null || !conn.channel().isActive()) {
                        evict(conn);
                        evicted++;
                        continue;
                    }

                    // MEM-01: Evict H2 connections that have exceeded max age AND have
                    // no active streams. Evicting an H2 connection with active streams
                    // would violate RFC 9113 — streams must complete or be reset first.
                    if (isExpired(conn, now) && conn.activeStreams() == 0) {
                        evict(conn);
                        conn.close();
                        stats.totalEvictions.incrementAndGet();
                        stats.totalConnectionsClosed.incrementAndGet();
                        evicted++;
                    }
                }
            }
        }

        if (evicted > 0) {
            logger.debug("Evicted {} connection(s) from pool (idle + max-age)", evicted);
        }
    }

    /**
     * MEM-01: Check if a connection has exceeded the max connection age.
     * Uses the connection's creation timestamp against the current time.
     */
    private boolean isExpired(HttpConnection conn) {
        return isExpired(conn, System.nanoTime());
    }

    /**
     * MEM-01: Check if a connection has exceeded the max connection age.
     *
     * @param conn the connection to check
     * @param now  current nanoTime (avoids repeated System.nanoTime() calls in sweep loops)
     */
    private boolean isExpired(HttpConnection conn, long now) {
        return now - conn.createdAtNanos() > maxConnectionAgeNanos;
    }

    /**
     * Close all connections in both pools and shut down the eviction executor.
     * Called when the frontend handler closes.
     */
    public void closeAll() {
        if (evictionTask != null) {
            evictionTask.cancel(false);
        }
        // PERF-POOL-01: Do not shut down the shared static executor — it is
        // shared across all ConnectionPool instances for the JVM lifetime.

        for (Deque<HttpConnection> deque : h1Pool.values()) {
            HttpConnection conn;
            while ((conn = deque.pollFirst()) != null) {
                stats.totalConnectionsClosed.incrementAndGet();
                conn.close();
            }
        }
        h1Pool.clear();
        h1PoolSize.clear();

        for (CopyOnWriteArrayList<HttpConnection> list : h2Pool.values()) {
            for (HttpConnection conn : list) {
                stats.totalConnectionsClosed.incrementAndGet();
                conn.close();
            }
            list.clear();
        }
        h2Pool.clear();
    }

    /**
     * STAT-01: Returns the pool statistics snapshot.
     *
     * @return the connection pool stats instance
     */
    public ConnectionPoolStats getStats() {
        return stats;
    }

    // ======================== Statistics ========================

    /**
     * STAT-01: Observable statistics for the connection pool. All counters are
     * thread-safe via AtomicLong. Provides both live counts (computed from pool state)
     * and cumulative counters (tracked via atomic increments).
     *
     * <p>Live counts (getActiveH1Connections, getIdleH1Connections, etc.) are computed
     * by scanning the pool structures. They are consistent snapshots within a single
     * pool (H1 or H2) but not atomic across both pools.</p>
     */
    public final class ConnectionPoolStats {

        private final AtomicLong totalConnectionsCreated = new AtomicLong(0);
        private final AtomicLong totalConnectionsClosed = new AtomicLong(0);
        private final AtomicLong totalEvictions = new AtomicLong(0);

        ConnectionPoolStats() {
            // Package-private constructor
        }

        /**
         * Returns the number of active (in-use) H1 connections across all nodes.
         * Note: H1 connections are removed from the idle pool when acquired, so
         * "active" here means connections that are currently serving requests and
         * not in the idle deque. This is an approximation — the exact count would
         * require tracking checked-out connections separately.
         */
        public int getActiveH1Connections() {
            return activeH1Count.get();
        }

        /**
         * Returns the number of active H2 connections (those with at least one stream).
         */
        public int getActiveH2Connections() {
            int count = 0;
            for (CopyOnWriteArrayList<HttpConnection> list : h2Pool.values()) {
                for (HttpConnection conn : list) {
                    if (conn.activeStreams() > 0) {
                        count++;
                    }
                }
            }
            return count;
        }

        /**
         * Returns the number of idle H1 connections across all nodes.
         */
        public int getIdleH1Connections() {
            int count = 0;
            for (java.util.concurrent.atomic.AtomicInteger sizeCounter : h1PoolSize.values()) {
                count += sizeCounter.get();
            }
            return count;
        }

        /**
         * Returns the number of idle H2 connections (those with zero active streams).
         */
        public int getIdleH2Connections() {
            int count = 0;
            for (CopyOnWriteArrayList<HttpConnection> list : h2Pool.values()) {
                for (HttpConnection conn : list) {
                    if (conn.activeStreams() == 0) {
                        count++;
                    }
                }
            }
            return count;
        }

        /**
         * Total number of connections created by this pool since creation.
         */
        public long getTotalConnectionsCreated() {
            return totalConnectionsCreated.get();
        }

        /**
         * Total number of connections closed by this pool since creation.
         */
        public long getTotalConnectionsClosed() {
            return totalConnectionsClosed.get();
        }

        /**
         * Total number of connections evicted (idle timeout or max-age) since creation.
         */
        public long getTotalEvictions() {
            return totalEvictions.get();
        }

        @Override
        public String toString() {
            return "ConnectionPoolStats{" +
                    "idleH1=" + getIdleH1Connections() +
                    ", activeH2=" + getActiveH2Connections() +
                    ", idleH2=" + getIdleH2Connections() +
                    ", totalCreated=" + totalConnectionsCreated.get() +
                    ", totalClosed=" + totalConnectionsClosed.get() +
                    ", totalEvictions=" + totalEvictions.get() +
                    '}';
        }
    }
}
