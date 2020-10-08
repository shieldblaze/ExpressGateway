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
package com.shieldblaze.expressgateway.core.server.udp;

import com.shieldblaze.expressgateway.core.l4.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.l4.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.CommonConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.loadbalance.l4.RoundRobin;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
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
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class UpstreamHandlerTest {

    static L4LoadBalancer l4LoadBalancer;
    static EventLoopFactory eventLoopFactory;

    @BeforeAll
    static void setup() {
        new UDPServer().start();

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

        CommonConfiguration commonConfiguration = CommonConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration.DEFAULT)
                .build();

        eventLoopFactory = new EventLoopFactory(commonConfiguration);

        Cluster cluster = new Cluster();
        cluster.setClusterName("MyCluster");
        cluster.addBackend(new Backend(new InetSocketAddress("127.0.0.1", 9111)));

        l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withCommonConfiguration(commonConfiguration)
                .withL4Balance(new RoundRobin())
                .withCluster(cluster)
                .withFrontListener(new UDPListener())
                .withBindAddress(new InetSocketAddress("127.0.0.1", 9110))
                .build();

        AtomicBoolean isStarted = new AtomicBoolean(false);


        System.out.println(l4LoadBalancer.start());

        l4LoadBalancer.start();
/*                .forEach(completableFuture -> {
            try {
                if (completableFuture.get().isSuccess()) {
                    isStarted.set(true);
                } else {
                    throw completableFuture.get().cause();
                }
            } catch (Throwable e) {
//                e.printStackTrace();
            }
        });*/

        assertTrue(isStarted.get());
    }

    @AfterAll
    static void stop() throws ExecutionException, InterruptedException {
        assertTrue(l4LoadBalancer.stop().get());
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
