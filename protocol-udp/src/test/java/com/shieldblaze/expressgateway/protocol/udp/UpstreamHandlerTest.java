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
package com.shieldblaze.expressgateway.protocol.udp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.EventLoopFactory;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import io.netty.channel.epoll.Epoll;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.util.Arrays;
import java.util.concurrent.ExecutionException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class UpstreamHandlerTest {

    static L4LoadBalancer l4LoadBalancer;
    static EventLoopFactory eventLoopFactory;

    @BeforeAll
    static void setup() throws InterruptedException, ExecutionException {
        new UDPServer().start();

        TransportConfiguration transportConfiguration = TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
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

        Cluster cluster = new ClusterPool(new EventStream(), new RoundRobin(new NOOPSessionPersistence()));
        cluster.addNode(new Node(cluster, new InetSocketAddress("127.0.0.1", 9111)));

        l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(coreConfiguration)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 9110))
                .withL4FrontListener(new UDPListener())
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = l4LoadBalancer.start();
        l4FrontListenerStartupEvent.future().get();
        assertTrue(l4FrontListenerStartupEvent.success());
    }

    @AfterAll
    static void stop() throws ExecutionException, InterruptedException {
        L4FrontListenerStopEvent l4FrontListenerStopEvent = l4LoadBalancer.stop();
        l4FrontListenerStopEvent.future().get();
        assertTrue(l4FrontListenerStopEvent.success());
    }

    @Test
    void udpClient() throws Exception {
        try (DatagramSocket datagramSocket = new DatagramSocket()) {
            DatagramPacket datagramPacket = new DatagramPacket("HELLO_FROM_CLIENT".getBytes(), "HELLO_FROM_CLIENT".length(),
                    InetAddress.getByName("127.0.0.1"), 9110);

            datagramSocket.send(datagramPacket);
            byte[] bytes = new byte[2048];
            datagramPacket = new DatagramPacket(bytes, bytes.length);
            datagramSocket.receive(datagramPacket);

            assertEquals("HELLO_FROM_SERVER", new String(Arrays.copyOf(datagramPacket.getData(), datagramPacket.getLength())));
        }
    }

    private static final class UDPServer extends Thread {

        @Override
        public void run() {
            try (DatagramSocket datagramSocket = new DatagramSocket(9111, InetAddress.getByName("127.0.0.1"))) {
                byte[] bytes = new byte[2048];
                DatagramPacket datagramPacket = new DatagramPacket(bytes, bytes.length);
                datagramSocket.receive(datagramPacket);

                InetAddress inetAddress = datagramPacket.getAddress();
                int port = datagramPacket.getPort();

                assertEquals("HELLO_FROM_CLIENT", new String(Arrays.copyOf(datagramPacket.getData(), datagramPacket.getLength())));

                datagramPacket = new DatagramPacket("HELLO_FROM_SERVER".getBytes(), "HELLO_FROM_SERVER".length(), inetAddress, port);
                datagramSocket.send(datagramPacket);
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
    }
}