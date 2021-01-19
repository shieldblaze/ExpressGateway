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
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class TCPLoadBalancerServiceTest {

    static Server server;
    static ManagedChannel channel;
    static String loadBalancerId;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = NettyServerBuilder.forAddress(new InetSocketAddress("127.0.0.1", 9110))
                .addService(new TCPLoadBalancerService())
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
    void simpleServerLBClientTest() throws IOException {
        new TCPServer().start();

        TCPLoadBalancerServiceGrpc.TCPLoadBalancerServiceBlockingStub tcpService = TCPLoadBalancerServiceGrpc.newBlockingStub(channel);
        NodeServiceGrpc.NodeServiceBlockingStub nodeService = NodeServiceGrpc.newBlockingStub(channel);

        LoadBalancer.TCPLoadBalancer tcpLoadBalancer = LoadBalancer.TCPLoadBalancer.newBuilder()
                .setBindAddress("127.0.0.1")
                .setBindPort(5000)
                .setName("Meow")
                .setUseDefaults(true)
                .build();

        LoadBalancer.LoadBalancerResponse loadBalancerResponse = tcpService.start(tcpLoadBalancer);
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

        try (Socket socket = new Socket("127.0.0.1", 5000)) {
            socket.getOutputStream().write("Meow".getBytes());
            assertArrayEquals("Cat".getBytes(), socket.getInputStream().readNBytes(3));
        } catch (IOException e) {
            throw e;
        }
    }

    @Test
    @Order(2)
    void getTest() {
        TCPLoadBalancerServiceGrpc.TCPLoadBalancerServiceBlockingStub tcpService = TCPLoadBalancerServiceGrpc.newBlockingStub(channel);
        LoadBalancer.GetLoadBalancerRequest request = LoadBalancer.GetLoadBalancerRequest.newBuilder()
                .setLoadBalancerId(loadBalancerId)
                .build();

        LoadBalancer.TCPLoadBalancer tcpLoadBalancer = tcpService.get(request);

        assertEquals("127.0.0.1", tcpLoadBalancer.getBindAddress());
        assertEquals(5000, tcpLoadBalancer.getBindPort());
    }

    @Test
    @Order(3)
    void stopTest() {
        TCPLoadBalancerServiceGrpc.TCPLoadBalancerServiceBlockingStub tcpService = TCPLoadBalancerServiceGrpc.newBlockingStub(channel);
        LoadBalancer.LoadBalancerResponse response = tcpService.stop(LoadBalancer.StopLoadBalancer.newBuilder().setId(loadBalancerId).build());
        assertEquals("Success", response.getResponseText());
    }

    private static final class TCPServer extends Thread {

        @Override
        public void run() {
            try (ServerSocket serverSocket = new ServerSocket(5555)) {
                Socket socket = serverSocket.accept();
                assertArrayEquals("Meow".getBytes(), socket.getInputStream().readNBytes(4));
                socket.getOutputStream().write("Cat".getBytes());
                Thread.sleep(1000L);
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
    }
}
