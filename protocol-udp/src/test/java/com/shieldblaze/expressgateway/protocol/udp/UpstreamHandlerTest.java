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
import com.shieldblaze.expressgateway.configuration.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.EventLoopFactory;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.util.Arrays;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class UpstreamHandlerTest {

    static L4LoadBalancer l4LoadBalancer;
    static EventLoopFactory eventLoopFactory;

    @BeforeAll
    static void setup() {
        new UDPServer().start();

        TransportConfiguration transportConfiguration = new TransportConfiguration()
                .transportType(TransportType.NIO)
                .tcpFastOpenMaximumPendingRequests(2147483647)
                .backendConnectTimeout(1000 * 5)
                .receiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .receiveBufferSizes(new int[]{100})
                .socketReceiveBufferSize(2147483647)
                .socketSendBufferSize(2147483647)
                .tcpConnectionBacklog(2147483647)
                .connectionIdleTimeout(180);

        EventLoopConfiguration eventLoopConfiguration = new EventLoopConfiguration()
                .parentWorkers(2)
                .childWorkers(4);

        CoreConfiguration coreConfiguration = CoreConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withBufferConfiguration(BufferConfiguration.DEFAULT)
                .build();

        eventLoopFactory = new EventLoopFactory(coreConfiguration);

        Cluster cluster = new ClusterPool(new EventStream(), new RoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 9111));

        l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(coreConfiguration)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 9110))
                .withL4FrontListener(new UDPListener())
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = l4LoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());
    }

    @AfterAll
    static void stop() {
        L4FrontListenerStopEvent l4FrontListenerStopEvent = l4LoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
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
