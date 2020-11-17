/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.healthcheckmanager.DefaultHealthCheckManager;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.TCPHealthCheck;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;

class ClusterPoolTest {

    static TCPServer tcpServer;

    @BeforeAll
    static void setup() throws InterruptedException {
        tcpServer = new TCPServer();
        tcpServer.start();

        Thread.sleep(2500L); // Wait for server to start
    }

    @Test
    void testBackendsHealth() throws InterruptedException {
        ClusterPool clusterPool = new ClusterPool();

        for (int i = 1; i < 100; i++) {
            HealthCheck healthCheck = new TCPHealthCheck(new InetSocketAddress("127.0.0.1", 10000), Duration.ofMillis(15));
            DefaultHealthCheckManager defaultHealthCheckManager = new DefaultHealthCheckManager(healthCheck, 1, 1, TimeUnit.SECONDS);
            clusterPool.addBackends(new Node(new InetSocketAddress("192.168.1." + i, i), 100, 100, healthCheck, defaultHealthCheckManager));
        }

        Thread.sleep(5000L); // Wait for all Health Checks to Finish

        for (Node node : clusterPool.onlineBackends()) {
            assertEquals(Health.GOOD, node.health());
        }

        assertEquals(99, clusterPool.onlineBackends().size());

        tcpServer.stop();
        Thread.sleep(10000L); // Wait for server to stop and all Health Checks to Finish

        for (Node node : clusterPool.onlineBackends()) {
            assertEquals(Health.BAD, node.health());
        }

        assertEquals(0, clusterPool.onlineBackends().size());
    }

    private static final class TCPServer {

        private final EventLoopGroup bossGroup = new NioEventLoopGroup(4);

        private void start() {
            ServerBootstrap serverBootstrap = new ServerBootstrap()
                    .group(bossGroup)
                    .channel(NioServerSocketChannel.class)
                    .childHandler(new ChannelInitializer<SocketChannel>() {
                        @Override
                        protected void initChannel(SocketChannel socketChannel) {
                            // Ignore
                        }
                    });

            serverBootstrap.bind(new InetSocketAddress("127.0.0.1", 10000));
        }

        private void stop() throws InterruptedException {
            bossGroup.shutdownGracefully().sync();
        }
    }
}
