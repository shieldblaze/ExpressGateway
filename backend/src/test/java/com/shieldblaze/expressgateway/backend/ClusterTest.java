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

import com.shieldblaze.expressgateway.healthcheck.Health;
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

import static org.junit.jupiter.api.Assertions.assertEquals;

class ClusterTest {

    static TCPServer tcpServer;

    @BeforeAll
    static void setup() throws InterruptedException {
        tcpServer = new TCPServer();
        tcpServer.start();

        Thread.sleep(2500L); // Wait for server to start
    }

    @Test
    void testBackendsHealth() throws InterruptedException {
        Cluster cluster = new Cluster();

        for (int i = 1; i < 100; i++) {
            cluster.addBackend(new Backend("localhost", new InetSocketAddress("192.168.1." + i, i), 100, 100,
                    new TCPHealthCheck(new InetSocketAddress("127.0.0.1", 10000), 5)));
        }

        Thread.sleep(2500L); // Wait for all Health Checks to Finish

        for (Backend backend : cluster.getBackends()) {
            assertEquals(Health.GOOD, backend.getHealth());
        }

        tcpServer.stop();
        Thread.sleep(5000L); // Wait for server to stop and all Health Checks to Finish

        for (Backend backend : cluster.getBackends()) {
            assertEquals(Health.BAD, backend.getHealth());
        }
    }

    private static final class TCPServer {

        private final EventLoopGroup bossGroup = new NioEventLoopGroup(2);
        private final EventLoopGroup workerGroup = new NioEventLoopGroup(4);

        private void start() {
            ServerBootstrap serverBootstrap = new ServerBootstrap()
                    .group(bossGroup, workerGroup)
                    .channel(NioServerSocketChannel.class)
                    .childHandler(new ChannelInitializer<SocketChannel>() {
                        @Override
                        protected void initChannel(SocketChannel socketChannel) {
                            // Ignore
                        }
                    });

            serverBootstrap.bind(new InetSocketAddress("127.0.0.1", 10000));
        }

        private void stop() {
            bossGroup.shutdownGracefully();
            workerGroup.shutdownGracefully();
        }
    }
}