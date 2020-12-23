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
package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.configuration.controller.ControllerConfiguration;
import com.shieldblaze.expressgateway.configuration.controller.ControllerConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfigurationBuilder;
import com.shieldblaze.expressgateway.core.controller.Controller;
import com.shieldblaze.expressgateway.healthcheck.l4.TCPHealthCheck;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.nio.NioSocketChannel;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.time.Duration;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NodeTest {

    static EventLoopGroup eventLoopGroup;

    @BeforeAll
    static void initEventLoopGroup() {
        eventLoopGroup = new NioEventLoopGroup(2);
    }

    @AfterAll
    static void stopEventLoopGroup() {
        eventLoopGroup.shutdownGracefully();
    }

    @SuppressWarnings("unchecked")
    @Test
    void testConnections() throws InterruptedException, TooManyConnectionsException {
        EventStreamConfiguration streamConfiguration = EventStreamConfigurationBuilder.newBuilder()
                .withWorkers(2)
                .build();

        ControllerConfiguration controllerConfiguration = ControllerConfigurationBuilder.newBuilder()
                .withOCSPCheckIntervalMilliseconds(100)
                .withHealthCheckIntervalMilliseconds(10)
                .withDeadConnectionCleanIntervalMilliseconds(10)
                .withWorkers(2)
                .build();

        Cluster cluster = new ClusterPool(streamConfiguration.eventStream(), new RoundRobin(NOOPSessionPersistence.INSTANCE), "TestPool");
        cluster.eventSubscriber().subscribe(new Controller(controllerConfiguration));

        // Start TCP Server
        TCPServer tcpServer = new TCPServer();
        tcpServer.start();
        Thread.sleep(500L);

        TCPHealthCheck healthCheck = new TCPHealthCheck(tcpServer.socketAddress, Duration.ofMillis(10));
        Node node = new Node(cluster, new InetSocketAddress("127.0.0.1", 1), 100, healthCheck);

        // Verify 0 connections in beginning
        assertEquals(0, node.activeConnection());
        assertEquals(0, node.activeConnection0());

        // Start 100 TCP Connections and connect to TCP Server
        for (int i = 0; i < 100; i++) {
            Connection connection = connection(node, tcpServer.socketAddress);
            node.addConnection(connection);
            connection.release();
        }

        // Verify 100 connections are active
        assertEquals(100, node.activeConnection());

        // Try connection 1 more connection which will cause maximum connection limit to exceed.
        assertThrows(TooManyConnectionsException.class, () -> node.addConnection(connection(node, tcpServer.socketAddress)));

        // Mark Node as Offline and shutdown TCP Server
        tcpServer.run.set(false);
        healthCheck.run();
        Thread.sleep(5000L);

        // Verify 0 connections are active
        assertEquals(0, node.activeConnection());
    }

    private Connection connection(Node node, InetSocketAddress socketAddress) {
        Bootstrap bootstrap = new Bootstrap()
                .group(eventLoopGroup)
                .channel(NioSocketChannel.class)
                .handler(new SimpleChannelInboundHandler<ByteBuf>() {
                    @Override
                    protected void channelRead0(ChannelHandlerContext ctx, ByteBuf msg) {
                        // Ignore
                    }
                });

        ChannelFuture channelFuture = bootstrap.connect(socketAddress);
        TCPConnection tcpConnection = new TCPConnection(node);
        tcpConnection.init(channelFuture);
        return tcpConnection;
    }

    private static final class TCPServer extends Thread {

        private final AtomicBoolean run = new AtomicBoolean(true);
        private InetSocketAddress socketAddress;

        @Override
        public void run() {
            try (ServerSocket serverSocket = new ServerSocket(0, 1000, InetAddress.getByName("127.0.0.1"))) {
                socketAddress = (InetSocketAddress) serverSocket.getLocalSocketAddress();
                while (run.get()) {
                    serverSocket.accept();
                }
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
    }

    private static final class TCPConnection extends Connection {

        private TCPConnection(Node node) {
            super(node, 1000);
        }

        @Override
        protected void processBacklog(ChannelFuture channelFuture) {
            // No Backlog
        }
    }
}
