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

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.backend.pool.Connection;
import com.shieldblaze.expressgateway.backend.services.BackendControllerService;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.nio.NioSocketChannel;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.net.ServerSocket;

import static org.junit.jupiter.api.Assertions.assertEquals;

class NodeTest {

    @Test
    void testNodeCreation() throws InterruptedException, TooManyConnectionsException {
        Cluster cluster = new ClusterPool(new EventStream(), new RoundRobin(new NOOPSessionPersistence()));

        Node node = new Node(cluster, new InetSocketAddress("127.0.0.1", 1));
        cluster.addNode(node);

        // Verify 0 connections in beginning
        assertEquals(0, node.activeConnection());
        assertEquals(0, node.activeConnection0());

        // Start TCP Server
        TCPServer tcpServer = new TCPServer();
        tcpServer.start();
        Thread.sleep(500L);

        // Start 100 TCP Connections and connect to TCP Server
        EventLoopGroup eventLoopGroup = new NioEventLoopGroup(2);
        for (int i = 0; i < 100; i++) {
            Bootstrap bootstrap = new Bootstrap()
                    .group(eventLoopGroup)
                    .channel(NioSocketChannel.class)
                    .handler(new SimpleChannelInboundHandler<ByteBuf>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, ByteBuf msg) {
                            // Ignore
                        }
                    });

            ChannelFuture channelFuture = bootstrap.connect("127.0.0.1", 9110);
            TCPConnection tcpConnection = new TCPConnection(1000);
            tcpConnection.init(channelFuture);
            node.addConnection(tcpConnection);
        }

        // Verify 100 connections are active
        assertEquals(100, node.activeConnection());

        // Mark Node as Offline and shutdown TCP Server
        node.state(State.OFFLINE);
        tcpServer.run = false;
        Thread.sleep(1000L);

        // Verify 0 connections are active
        assertEquals(0, node.activeConnection());
    }

    private static final class TCPServer extends Thread {

        private boolean run;

        @Override
        public void run() {
            try (ServerSocket serverSocket = new ServerSocket(9110, 1000)) {
                while (run) {
                    serverSocket.accept();
                }
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
    }

    private static final class TCPConnection extends Connection {

        private TCPConnection(long timeout) {
            super(timeout);
        }

        @Override
        protected void processBacklog(ChannelFuture channelFuture) {
            // No Backlog
        }
    }
}
