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
package com.shieldblaze.expressgateway.core.server.tcp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.SingleBackendCluster;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.EventLoopFactory;
import io.netty.channel.epoll.Epoll;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class UpstreamHandlerTest {

    static L4LoadBalancer l4LoadBalancer;
    static EventLoopFactory eventLoopFactory;

    @BeforeAll
    static void setup() throws InterruptedException {
        new TCPServer().start();

        TransportConfiguration transportConfiguration = TransportConfigurationBuilder.newBuilder()
                .withTransportType(Epoll.isAvailable() ? TransportType.EPOLL : TransportType.NIO)
                .withTCPFastOpenMaximumPendingRequests(2147483647)
                .withBackendConnectTimeout(1000 * 5)
                .withBackendSocketTimeout(1000 * 5)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{100})
                .withSocketReceiveBufferSize(2147483647)
                .withSocketSendBufferSize(2147483647)
                .withTCPConnectionBacklog(2147483647)
                .withDataBacklog(2147483647)
                .withConnectionIdleTimeout(180)
                .build();

        EventLoopConfiguration eventLoopConfiguration = EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(Runtime.getRuntime().availableProcessors())
                .withChildWorkers(Runtime.getRuntime().availableProcessors() * 2)
                .build();

        CoreConfiguration coreConfiguration = CoreConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration.DEFAULT)
                .build();

        eventLoopFactory = new EventLoopFactory(coreConfiguration);

        Cluster cluster = SingleBackendCluster.of((new Node(new InetSocketAddress("127.0.0.1", 9111))));

        l4LoadBalancer = DefaultL4LoadBalancerBuilder.newBuilder()
                .withCommonConfiguration(coreConfiguration)
                .withL4Balance(new RoundRobin())
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 9110))
                .withFrontListener(new TCPListener())
                .build();

        List<CompletableFuture<L4FrontListenerStartupEvent>> list = l4LoadBalancer.start();
        Thread.sleep(2500L);
        for (CompletableFuture<L4FrontListenerStartupEvent> completableFuture : list) {
            try {
                assertTrue(completableFuture.get().channelFuture().isSuccess());
            } catch (Throwable e) {
                e.printStackTrace();
            }
        }
    }

    @AfterAll
    static void stop() throws ExecutionException, InterruptedException {
        assertTrue(l4LoadBalancer.stop().get());
    }

    @Test
    void tcpClient() throws Exception {
        try (Socket client = new Socket("127.0.0.1", 9110)) {
            DataInputStream in = new DataInputStream(client.getInputStream());
            DataOutputStream out = new DataOutputStream(client.getOutputStream());

            out.writeUTF("HELLO_FROM_CLIENT");
            out.flush();

            Thread.sleep(2500L); // Wait for server to respond

            assertEquals("HELLO_FROM_SERVER", in.readUTF());
        }
    }

    private static final class TCPServer extends Thread {

        @Override
        public void run() {
            try (ServerSocket serverSocket = new ServerSocket(9111, 1000, InetAddress.getByName("127.0.0.1"))) {
                Socket clientSocket = serverSocket.accept();
                DataInputStream input = new DataInputStream(clientSocket.getInputStream());
                DataOutputStream out = new DataOutputStream(clientSocket.getOutputStream());

                assertEquals("HELLO_FROM_CLIENT", input.readUTF());

                out.writeUTF("HELLO_FROM_SERVER");
                out.flush();
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
    }
}
