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
package com.shieldblaze.expressgateway.backend.healthcheck;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.healthcheck.Health;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;

/**
 * P1 test: Verifies that a node transitions from healthy (GOOD) to unhealthy (BAD)
 * after consecutive health check failures.
 *
 * <p>This test starts a TCP server, configures health checks against it,
 * waits for the node to become healthy, then shuts down the server and
 * verifies the node transitions to an unhealthy state.</p>
 */
@Timeout(value = 60, unit = TimeUnit.SECONDS)
class HealthCheckFailureTransitionTest {

    @Test
    void nodeTransitionsFromHealthyToUnhealthy() throws Exception {
        int port = AvailablePortUtil.getTcpPort();

        EventLoopGroup eventLoopGroup = new NioEventLoopGroup(1);
        try {
            // Start a simple TCP echo server
            Channel serverChannel = new ServerBootstrap()
                    .group(eventLoopGroup)
                    .channel(NioServerSocketChannel.class)
                    .childHandler(new ChannelInitializer<SocketChannel>() {
                        @Override
                        protected void initChannel(SocketChannel ch) {
                            ch.pipeline().addLast(new SimpleChannelInboundHandler<ByteBuf>() {
                                @Override
                                protected void channelRead0(ChannelHandlerContext ctx, ByteBuf msg) {
                                    ctx.writeAndFlush(msg.retainedDuplicate());
                                }
                            });
                        }
                    })
                    .bind("127.0.0.1", port).sync().channel();

            HealthCheckTemplate healthCheckTemplate = HealthCheckTemplate.builder()
                    .protocol(HealthCheckTemplate.Protocol.TCP)
                    .host("127.0.0.1")
                    .port(port)
                    .path("")
                    .timeout(1)
                    .samples(5)
                    .build();

            Cluster cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                    .withHealthCheck(HealthCheckConfiguration.DEFAULT, healthCheckTemplate)
                    .build();

            Node node = NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", port))
                    .build();

            cluster.addNode(node);

            // Wait for node to become healthy
            waitForHealth(node, Health.GOOD, 15_000);
            assertEquals(Health.GOOD, node.health(), "Node should be healthy when server is up");

            // Shut down the server -- health checks should start failing
            serverChannel.close().sync();

            // Wait for health to transition to BAD
            waitForHealth(node, Health.BAD, 20_000);
            assertEquals(Health.BAD, node.health(), "Node should be unhealthy after server shutdown");

            // Verify state transitioned to OFFLINE
            assertEquals(State.OFFLINE, node.state(), "Node state should be OFFLINE when health is BAD");

            // Clean up
            cluster.close();
        } finally {
            eventLoopGroup.shutdownGracefully().sync();
        }
    }

    /**
     * Wait for a node to reach the expected health status, polling periodically.
     */
    private void waitForHealth(Node node, Health expectedHealth, long timeoutMs) throws InterruptedException {
        long deadline = System.currentTimeMillis() + timeoutMs;
        while (System.currentTimeMillis() < deadline) {
            if (node.health() == expectedHealth) {
                return;
            }
            Thread.sleep(500);
        }
    }
}
