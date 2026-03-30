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
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link ConnectionPool}.
 *
 * <p>Validates pool creation, acquire/release semantics, pool exhaustion,
 * and connection reuse for HTTP/1.1 connections.</p>
 */
class ConnectionPoolTest {

    private static EventLoopGroup bossGroup;
    private static EventLoopGroup workerGroup;
    private static int serverPort;
    private static Channel serverChannel;

    private ConnectionPool pool;
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
        pool = new ConnectionPool(HttpConfiguration.DEFAULT);

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", serverPort))
                .build();

        openChannels.clear();
    }

    /**
     * Test that acquireH1 returns null when pool is empty.
     */
    @Test
    void acquireH1ReturnsNullWhenEmpty() {
        HttpConnection conn = pool.acquireH1(node);
        assertNull(conn, "Pool should return null when no connections are available");
    }

    /**
     * Test pool creation and basic release + acquire cycle.
     * A released connection should be acquirable again.
     */
    @Test
    void releaseAndAcquireH1() throws Exception {
        HttpConnection conn = createLiveH1Connection();

        // Release the connection to the pool
        pool.releaseH1(node, conn);

        // Acquire should return the same connection
        HttpConnection acquired = pool.acquireH1(node);
        assertNotNull(acquired, "Should acquire a pooled connection");
        assertSame(conn, acquired, "Should get the same connection back");
    }

    /**
     * Test that released connections can be reused (LIFO ordering).
     */
    @Test
    void releasedConnectionsAreReusedLifo() throws Exception {
        HttpConnection conn1 = createLiveH1Connection();
        HttpConnection conn2 = createLiveH1Connection();

        // Release both connections
        pool.releaseH1(node, conn1);
        pool.releaseH1(node, conn2);

        // LIFO: most recently released should come out first
        HttpConnection first = pool.acquireH1(node);
        assertNotNull(first, "Should acquire first pooled connection");
        assertSame(conn2, first, "LIFO: most recently released should be acquired first");

        HttpConnection second = pool.acquireH1(node);
        assertNotNull(second, "Should acquire second pooled connection");
        assertSame(conn1, second, "Second acquire should return the other connection");
    }

    /**
     * Test that acquireH1 returns null after pool is drained.
     */
    @Test
    void acquireReturnsNullAfterDrain() throws Exception {
        HttpConnection conn = createLiveH1Connection();

        pool.releaseH1(node, conn);

        // Acquire the only connection
        HttpConnection acquired = pool.acquireH1(node);
        assertNotNull(acquired);

        // Pool should now be empty
        HttpConnection empty = pool.acquireH1(node);
        assertNull(empty, "Pool should be empty after draining all connections");
    }

    /**
     * Test that evicting a connection removes it from the pool.
     */
    @Test
    void evictRemovesConnection() throws Exception {
        HttpConnection conn = createLiveH1Connection();

        pool.releaseH1(node, conn);
        pool.evict(conn);

        HttpConnection acquired = pool.acquireH1(node);
        assertNull(acquired, "Evicted connection should not be acquirable");
    }

    /**
     * Test that closeAll closes all pooled connections.
     */
    @Test
    void closeAllClearsPool() throws Exception {
        HttpConnection conn = createLiveH1Connection();

        pool.releaseH1(node, conn);
        pool.closeAll();

        HttpConnection acquired = pool.acquireH1(node);
        assertNull(acquired, "Pool should be empty after closeAll");
    }

    /**
     * Test allActiveConnections returns pooled connections.
     */
    @Test
    void allActiveConnectionsReturnsPooledConnections() throws Exception {
        HttpConnection conn1 = createLiveH1Connection();
        HttpConnection conn2 = createLiveH1Connection();

        pool.releaseH1(node, conn1);
        pool.releaseH1(node, conn2);

        assertTrue(pool.allActiveConnections().size() >= 2,
                "allActiveConnections should include all pooled connections");
    }

    /**
     * Creates an HttpConnection backed by a real TCP channel connected to the
     * test server. Uses {@code init()} to properly set up the Connection state.
     */
    private HttpConnection createLiveH1Connection() throws Exception {
        HttpConnection conn = new HttpConnection(node, HttpConfiguration.DEFAULT);

        // Create a real connection to the test server
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
