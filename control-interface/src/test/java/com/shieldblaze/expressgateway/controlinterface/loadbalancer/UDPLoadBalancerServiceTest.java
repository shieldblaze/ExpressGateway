/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

package com.shieldblaze.expressgateway.controlinterface.loadbalancer;

import com.shieldblaze.expressgateway.controlinterface.node.NodeOuterClass;
import com.shieldblaze.expressgateway.controlinterface.node.NodeService;
import com.shieldblaze.expressgateway.controlinterface.node.NodeServiceGrpc;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Server;
import io.grpc.netty.shaded.io.grpc.netty.NettyServerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.Arrays;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.*;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class UDPLoadBalancerServiceTest {

    static Server server;
    static ManagedChannel channel;
    static String loadBalancerId;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = NettyServerBuilder.forAddress(new InetSocketAddress("127.0.0.1", 9110))
                .addService(new UDPLoadBalancerService())
                .addService(new NodeService())
                .build()
                .start();

        channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();
    }

    @AfterAll
    static void shutdown() throws InterruptedException {
        channel.shutdownNow();
        server.shutdownNow().awaitTermination(30, TimeUnit.SECONDS);
    }

    @Test
    @Order(1)
    void simpleServerLBClientTest() throws IOException, InterruptedException {
        new UDPServer(false, 5555).start();

        UDPLoadBalancerServiceGrpc.UDPLoadBalancerServiceBlockingStub udpService = UDPLoadBalancerServiceGrpc.newBlockingStub(channel);
        NodeServiceGrpc.NodeServiceBlockingStub nodeService = NodeServiceGrpc.newBlockingStub(channel);

        LoadBalancer.UDPLoadBalancer tcpLoadBalancer = LoadBalancer.UDPLoadBalancer.newBuilder()
                .setBindAddress("127.0.0.1")
                .setBindPort(5000)
                .setName("Meow")
                .setUseDefaults(true)
                .build();

        LoadBalancer.LoadBalancerResponse loadBalancerResponse = udpService.start(tcpLoadBalancer);
        assertFalse(loadBalancerResponse.getResponseText().isEmpty()); // Load Balancer ID must exist

        loadBalancerId = loadBalancerResponse.getResponseText();

        NodeOuterClass.AddRequest addRequest = NodeOuterClass.AddRequest.newBuilder()
                .setAddress("127.0.0.1")
                .setPort(5555)
                .setLoadBalancerID(loadBalancerResponse.getResponseText())
                .setMaxConnections(-1)
                .build();

        NodeOuterClass.AddResponse addResponse = nodeService.add(addRequest);
        assertTrue(addResponse.getSuccess());
        assertFalse(addResponse.getNodeId().isEmpty()); // Load Balancer ID

        DatagramSocket datagramSocket = new DatagramSocket();
        datagramSocket.send(new DatagramPacket(PING, 0, 4, InetAddress.getByName("127.0.0.1"), 5000));
        DatagramPacket datagramPacket = new DatagramPacket(new byte[4], 4);
        datagramSocket.receive(datagramPacket);

        assertArrayEquals(PONG, datagramPacket.getData());

        Thread.sleep(2500L); // Wait for everything to settle down
    }

    @Test
    @Order(2)
    void getTest() {
        UDPLoadBalancerServiceGrpc.UDPLoadBalancerServiceBlockingStub udpService = UDPLoadBalancerServiceGrpc.newBlockingStub(channel);
        LoadBalancer.GetLoadBalancerRequest request = LoadBalancer.GetLoadBalancerRequest.newBuilder()
                .setLoadBalancerId(loadBalancerId)
                .build();

        LoadBalancer.UDPLoadBalancer udpLoadBalancer = udpService.get(request);

        assertEquals("127.0.0.1", udpLoadBalancer.getBindAddress());
        assertEquals(5000, udpLoadBalancer.getBindPort());
    }

    @Test
    @Order(3)
    void stopTest() {
        UDPLoadBalancerServiceGrpc.UDPLoadBalancerServiceBlockingStub udpService = UDPLoadBalancerServiceGrpc.newBlockingStub(channel);
        LoadBalancer.LoadBalancerResponse response = udpService.stop(LoadBalancer.StopLoadBalancer.newBuilder().setId(loadBalancerId).build());
        assertEquals("Success", response.getResponseText());
    }

    private static final byte[] PING = "PING".getBytes();
    private static final byte[] PONG = "PONG".getBytes();

    private static final class UDPServer extends Thread {

        private final boolean ping;
        private final int port;

        private UDPServer(boolean ping, int port) {
            this.ping = ping;
            this.port = port;
        }

        @Override
        public void run() {
            try (DatagramSocket datagramSocket = new DatagramSocket(port, InetAddress.getByName("127.0.0.1"))) {
                byte[] bytes = new byte[2048];
                DatagramPacket datagramPacket = new DatagramPacket(bytes, bytes.length);
                datagramSocket.receive(datagramPacket);

                InetAddress inetAddress = datagramPacket.getAddress();
                int port = datagramPacket.getPort();

                assertArrayEquals(PING, Arrays.copyOf(datagramPacket.getData(), datagramPacket.getLength()));

                if (ping) {
                    datagramPacket = new DatagramPacket(PING, 4, inetAddress, port);
                } else {
                    datagramPacket = new DatagramPacket(PONG, 4, inetAddress, port);
                }

                datagramSocket.send(datagramPacket);
            } catch (Exception ex) {
                // Ignore
            }
        }
    }
}
