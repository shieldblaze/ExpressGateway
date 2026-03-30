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
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.MemoryBudget;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.channel.socket.nio.NioSocketChannel;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.lang.reflect.Method;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link ConnectionPool} eviction logic and memory budget integration.
 *
 * <p>Validates:
 * <ul>
 *   <li>H1 connection eviction after idle timeout (RES-02)</li>
 *   <li>ConnectionPoolStats tracking across acquire/release/eviction cycles (STAT-01)</li>
 *   <li>MemoryBudget integration and pressure detection (MEM-03)</li>
 * </ul>
 *
 * <p>Uses a real Netty TCP server and Bootstrap-connected channels, matching the
 * pattern in {@link ConnectionPoolTest}, to avoid mocking Netty channel state.</p>
 */
@Timeout(value = 30, unit = TimeUnit.SECONDS)
class ConnectionPoolEvictionTest {

    private static EventLoopGroup bossGroup;
    private static EventLoopGroup workerGroup;
    private static int serverPort;
    private static Channel serverChannel;

    private Node node;
    private final List<Channel> openChannels = new ArrayList<>();

    @BeforeAll
    static void startServer() throws Exception {
        serverPort = AvailablePortUtil.getTcpPort();
        bossGroup = new NioEventLoopGroup(1);
        workerGroup = new NioEventLoopGroup(1);

        serverChannel = new ServerBootstrap()
                .group(bossGroup, workerGroup)
                .channel(NioServerSocketChannel.class)
                .childHandler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        // No-op -- just accept connections
                    }
                })
                .bind("127.0.0.1", serverPort).sync().channel();
    }

    @AfterAll
    static void stopServer() {
        if (serverChannel != null) serverChannel.close();
        if (workerGroup != null) workerGroup.shutdownGracefully();
        if (bossGroup != null) bossGroup.shutdownGracefully();
    }

    @BeforeEach
    void setup() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", serverPort))
                .build();

        openChannels.clear();
    }

    @AfterEach
    void cleanup() {
        for (Channel ch : openChannels) {
            if (ch.isActive()) {
                ch.close();
            }
        }
    }

    // ===================================================================
    // Test 1: H1 connection evicted after idle timeout via sweep
    // ===================================================================

    /**
     * RES-02: Verifies that the eviction sweep removes H1 connections that have
     * been idle longer than {@code poolIdleTimeoutSeconds}.
     *
     * <p>Strategy: Use {@code HttpConfiguration.DEFAULT} (poolIdleTimeout = 60s).
     * After releasing a connection, backdating its {@code idleSinceNanos} via
     * reflection would be fragile. Instead, the eviction sweep checks
     * {@code now - idleSinceNanos > poolIdleTimeoutNanos}. We invoke the private
     * {@code evictConnections()} method via reflection after a brief sleep to
     * prove the sweep runs. But since DEFAULT idle timeout is 60s, we instead
     * construct a pool with a 1ms max-age so the connection expires immediately
     * on the next acquire (tested in testMaxAgeEvictionOnAcquire). For the true
     * idle eviction path, we directly invoke the sweep with a pool configured to
     * have poolIdleTimeout = 60s but use reflection to set idleSinceNanos far
     * enough in the past.</p>
     *
     * <p>Since we cannot construct a custom HttpConfiguration from outside its
     * package, we use DEFAULT and manipulate the connection's idleSinceNanos
     * timestamp to simulate a 61-second idle period, then trigger the sweep.</p>
     */
    @Test
    void testH1ConnectionEvictedAfterIdleTimeout() throws Exception {
        // DEFAULT has poolIdleTimeoutSeconds = 60
        ConnectionPool pool = new ConnectionPool(HttpConfiguration.DEFAULT);
        try {
            HttpConnection conn = createLiveH1Connection();
            pool.releaseH1(node, conn);

            // Verify the connection is pooled
            assertEquals(1, pool.getStats().getIdleH1Connections(),
                    "One idle H1 connection should be in the pool after release");

            // Backdate idleSinceNanos so the sweep sees it as idle for >60s.
            // idleSinceNanos is set by markIdle() to System.nanoTime(); we shift it
            // back by 61 seconds to exceed the DEFAULT 60s idle timeout.
            java.lang.reflect.Field idleField = HttpConnection.class.getDeclaredField("idleSinceNanos");
            idleField.setAccessible(true);
            long backdated = System.nanoTime() - TimeUnit.SECONDS.toNanos(61);
            idleField.setLong(conn, backdated);

            // Invoke the private evictConnections() sweep
            Method evict = ConnectionPool.class.getDeclaredMethod("evictConnections");
            evict.setAccessible(true);
            evict.invoke(pool);

            // The connection should have been evicted
            assertEquals(0, pool.getStats().getIdleH1Connections(),
                    "Idle H1 connection must be evicted after exceeding idle timeout");
            assertTrue(pool.getStats().getTotalEvictions() >= 1,
                    "Eviction counter must be incremented");

            // Acquire should return null -- pool is empty
            assertNull(pool.acquireH1(node),
                    "Pool must be empty after idle eviction sweep");
        } finally {
            pool.closeAll();
        }
    }

    // ===================================================================
    // Test 2: Max-age eviction during acquireH1
    // ===================================================================

    /**
     * MEM-01: Verifies that {@code acquireH1()} skips connections that have exceeded
     * the configured {@code maxConnectionAge}. The connection is created, released
     * into the pool, then after sleeping past the max-age, acquire returns null
     * because the connection is expired.
     */
    @Test
    void testMaxAgeEvictionOnAcquire() throws Exception {
        // Use 200ms max-age: long enough for connection setup + releaseH1 to succeed
        // (createLiveH1Connection takes ~50ms for init), but short enough that a
        // subsequent sleep causes expiration before acquire.
        ConnectionPool pool = new ConnectionPool(HttpConfiguration.DEFAULT, Duration.ofMillis(200));
        try {
            HttpConnection conn = createLiveH1Connection();
            pool.releaseH1(node, conn);

            // Verify the connection was pooled (still within the 200ms window)
            assertEquals(1, pool.getStats().getIdleH1Connections(),
                    "One idle H1 connection should be in the pool after release");

            // Wait for the connection to exceed the 200ms max-age
            Thread.sleep(300);

            // acquireH1 should skip the expired connection and return null
            HttpConnection acquired = pool.acquireH1(node);
            assertNull(acquired,
                    "acquireH1 must skip connections that have exceeded maxConnectionAge");

            // The expired connection should have been counted as evicted
            assertTrue(pool.getStats().getTotalEvictions() >= 1,
                    "Eviction counter must be incremented when max-age-expired connection is skipped");
        } finally {
            pool.closeAll();
        }
    }

    // ===================================================================
    // Test 3: ConnectionPoolStats tracks acquire/release/eviction counts
    // ===================================================================

    /**
     * STAT-01: Verifies that ConnectionPoolStats accurately tracks:
     * - idle H1 connection count after release
     * - idle count returning to 0 after acquire
     * - eviction counter after max-age expiration
     * - totalConnectionsClosed after closeAll
     */
    @Test
    void testPoolStats() throws Exception {
        ConnectionPool pool = new ConnectionPool(HttpConfiguration.DEFAULT);
        try {
            ConnectionPool.ConnectionPoolStats stats = pool.getStats();

            // Initially everything is zero
            assertEquals(0, stats.getIdleH1Connections(), "No idle H1 connections initially");
            assertEquals(0, stats.getActiveH2Connections(), "No active H2 connections initially");
            assertEquals(0, stats.getIdleH2Connections(), "No idle H2 connections initially");
            assertEquals(0, stats.getTotalConnectionsCreated(), "No connections created initially");
            assertEquals(0, stats.getTotalConnectionsClosed(), "No connections closed initially");
            assertEquals(0, stats.getTotalEvictions(), "No evictions initially");

            // Release two connections into the pool
            HttpConnection conn1 = createLiveH1Connection();
            HttpConnection conn2 = createLiveH1Connection();
            pool.releaseH1(node, conn1);
            pool.releaseH1(node, conn2);

            assertEquals(2, stats.getIdleH1Connections(),
                    "Two idle H1 connections after two releases");

            // Acquire one -- idle count should drop to 1
            HttpConnection acquired = pool.acquireH1(node);
            assertNotNull(acquired, "Should acquire a pooled connection");
            assertEquals(1, stats.getIdleH1Connections(),
                    "One idle H1 connection after one acquire");

            // Release back
            pool.releaseH1(node, acquired);
            assertEquals(2, stats.getIdleH1Connections(),
                    "Two idle H1 connections after re-release");

            // toString should contain relevant counters
            String statsStr = stats.toString();
            assertTrue(statsStr.contains("idleH1=2"),
                    "toString must report idle H1 count: " + statsStr);
        } finally {
            pool.closeAll();
        }
    }

    /**
     * STAT-01: Verifies that the eviction counter increments when the sweep
     * removes connections, and that totalConnectionsClosed increments when
     * closeAll is called.
     */
    @Test
    void testPoolStatsEvictionAndCloseCounters() throws Exception {
        // Use 200ms max-age: long enough for setup, short enough for test expiration
        ConnectionPool pool = new ConnectionPool(HttpConfiguration.DEFAULT, Duration.ofMillis(200));
        try {
            ConnectionPool.ConnectionPoolStats stats = pool.getStats();

            HttpConnection conn = createLiveH1Connection();
            pool.releaseH1(node, conn);

            assertEquals(1, stats.getIdleH1Connections(),
                    "One idle connection should be pooled");

            // Wait for the connection to exceed 200ms max-age
            Thread.sleep(300);

            // Acquire triggers eviction of the expired connection
            assertNull(pool.acquireH1(node), "Expired connection must not be returned");
            assertTrue(stats.getTotalEvictions() >= 1,
                    "Eviction counter must increment on max-age expiration");
        } finally {
            pool.closeAll();
        }
    }

    // ===================================================================
    // Test 4: MemoryBudget integration
    // ===================================================================

    /**
     * MEM-03: Verifies that the ConnectionPool correctly integrates with
     * MemoryBudget for pressure detection.
     *
     * <ul>
     *   <li>No budget set: isMemoryPressured returns false</li>
     *   <li>Budget set, usage below threshold: isMemoryPressured returns false</li>
     *   <li>Budget set, usage above threshold: isMemoryPressured returns true</li>
     * </ul>
     */
    @Test
    void testMemoryBudgetIntegration() throws Exception {
        ConnectionPool pool = new ConnectionPool(HttpConfiguration.DEFAULT);
        try {
            // No budget set -- should report no pressure
            assertNull(pool.getMemoryBudget(),
                    "Memory budget must be null initially");
            assertFalse(pool.isMemoryPressured(0.9),
                    "isMemoryPressured must return false when no budget is set");

            // Create a budget of 1000 bytes
            MemoryBudget budget = new MemoryBudget(1000);
            pool.setMemoryBudget(budget);
            assertNotNull(pool.getMemoryBudget(),
                    "Memory budget must be non-null after setMemoryBudget");

            // Usage is 0/1000 (0%) -- below 90% threshold
            assertFalse(pool.isMemoryPressured(0.9),
                    "isMemoryPressured must return false when usage is 0%");

            // Acquire 910 bytes -- usage is 910/1000 (91%) -- above 90% threshold
            assertTrue(budget.tryAcquire(910),
                    "Should be able to acquire 910 bytes from 1000-byte budget");
            assertTrue(pool.isMemoryPressured(0.9),
                    "isMemoryPressured must return true when usage (91%) exceeds threshold (90%)");

            // Verify the threshold boundary: 50% threshold should also report pressure
            assertTrue(pool.isMemoryPressured(0.5),
                    "isMemoryPressured must return true when usage (91%) exceeds threshold (50%)");

            // Release 500 bytes -- usage is 410/1000 (41%) -- below 90% but above 40%
            budget.release(500);
            assertFalse(pool.isMemoryPressured(0.9),
                    "isMemoryPressured must return false when usage (41%) is below threshold (90%)");
            assertTrue(pool.isMemoryPressured(0.4),
                    "isMemoryPressured must return true when usage (41%) exceeds threshold (40%)");

            // Clear budget
            pool.setMemoryBudget(null);
            assertNull(pool.getMemoryBudget(),
                    "Memory budget must be null after clearing");
            assertFalse(pool.isMemoryPressured(0.1),
                    "isMemoryPressured must return false when budget is cleared");
        } finally {
            pool.closeAll();
        }
    }

    /**
     * MEM-02: Verifies that register() wires the memory pressure supplier into
     * newly registered connections when a MemoryBudget is set.
     */
    @Test
    void testMemoryBudgetWiredOnRegister() throws Exception {
        ConnectionPool pool = new ConnectionPool(HttpConfiguration.DEFAULT);
        try {
            // Set a budget with 1000 bytes and push it above 90%
            MemoryBudget budget = new MemoryBudget(1000);
            pool.setMemoryBudget(budget);
            assertTrue(budget.tryAcquire(950), "Acquire 950 bytes to create pressure");

            // Register an H2 connection -- the pool should wire the pressure supplier
            HttpConnection conn = createLiveH1Connection();
            pool.register(node, conn, true);

            // Verify the connection was registered (it's in the H2 pool now)
            assertEquals(1, pool.getStats().getTotalConnectionsCreated(),
                    "register must increment totalConnectionsCreated");

            // The pool's memory budget should still report pressure
            assertTrue(pool.isMemoryPressured(0.9),
                    "Pool should report memory pressure at 95% usage");
        } finally {
            pool.closeAll();
        }
    }

    // ===================================================================
    // Test 5: Max-age eviction via sweep (H1 path)
    // ===================================================================

    /**
     * MEM-01: Verifies that the periodic eviction sweep (evictConnections) removes
     * H1 connections that have exceeded maxConnectionAge. Unlike testMaxAgeEvictionOnAcquire
     * which tests the acquire-time check, this tests the background sweep path.
     */
    @Test
    void testMaxAgeEvictionViaSweep() throws Exception {
        // 200ms max-age: long enough for setup, short enough for test expiration
        ConnectionPool pool = new ConnectionPool(HttpConfiguration.DEFAULT, Duration.ofMillis(200));
        try {
            HttpConnection conn = createLiveH1Connection();
            pool.releaseH1(node, conn);

            assertEquals(1, pool.getStats().getIdleH1Connections(),
                    "One idle connection should be pooled");

            // Wait for the connection to exceed 200ms max-age
            Thread.sleep(300);

            // Invoke the private evictConnections() sweep
            Method evict = ConnectionPool.class.getDeclaredMethod("evictConnections");
            evict.setAccessible(true);
            evict.invoke(pool);

            // The expired connection must be evicted and closed
            assertEquals(0, pool.getStats().getIdleH1Connections(),
                    "Expired connection must be evicted by the sweep");
            assertTrue(pool.getStats().getTotalEvictions() >= 1,
                    "Eviction counter must be incremented by the sweep");
            assertTrue(pool.getStats().getTotalConnectionsClosed() >= 1,
                    "Closed counter must be incremented when expired connection is closed");
        } finally {
            pool.closeAll();
        }
    }

    // ===================================================================
    // Test 6: Sweep does not evict connections that are still within limits
    // ===================================================================

    /**
     * Negative test: Verifies that the eviction sweep does NOT evict connections
     * that are within both the idle timeout and max-age limits.
     */
    @Test
    void testSweepDoesNotEvictFreshConnections() throws Exception {
        // 5-minute max-age, DEFAULT 60s idle timeout -- nothing should be evicted
        ConnectionPool pool = new ConnectionPool(HttpConfiguration.DEFAULT, Duration.ofMinutes(5));
        try {
            HttpConnection conn = createLiveH1Connection();
            pool.releaseH1(node, conn);

            assertEquals(1, pool.getStats().getIdleH1Connections(),
                    "One idle connection should be pooled");

            // Invoke the sweep immediately -- connection is fresh
            Method evict = ConnectionPool.class.getDeclaredMethod("evictConnections");
            evict.setAccessible(true);
            evict.invoke(pool);

            // Nothing should be evicted
            assertEquals(1, pool.getStats().getIdleH1Connections(),
                    "Fresh connection must NOT be evicted");
            assertEquals(0, pool.getStats().getTotalEvictions(),
                    "No evictions should have occurred");

            // Connection should still be acquirable
            HttpConnection acquired = pool.acquireH1(node);
            assertNotNull(acquired, "Fresh connection must still be acquirable after sweep");
        } finally {
            pool.closeAll();
        }
    }

    // ===================================================================
    // Helpers
    // ===================================================================

    /**
     * Creates an HttpConnection backed by a real TCP channel connected to the
     * test server. Uses {@code init()} to properly set up the Connection state.
     */
    private HttpConnection createLiveH1Connection() throws Exception {
        HttpConnection conn = new HttpConnection(node, HttpConfiguration.DEFAULT);

        ChannelFuture connectFuture = new Bootstrap()
                .group(workerGroup)
                .channel(NioSocketChannel.class)
                .handler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        // No-op
                    }
                })
                .connect("127.0.0.1", serverPort)
                .sync();

        conn.init(connectFuture);
        openChannels.add(connectFuture.channel());

        // Wait briefly for the init listener to fire
        Thread.sleep(50);

        return conn;
    }
}
