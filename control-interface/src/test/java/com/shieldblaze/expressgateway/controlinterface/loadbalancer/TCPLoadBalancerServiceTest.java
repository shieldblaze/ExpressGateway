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

import com.shieldblaze.expressgateway.controlinterface.loadbalancer.Layer4LoadBalancer.LoadBalancerResponse;
import com.shieldblaze.expressgateway.controlinterface.node.NodeOuterClass;
import com.shieldblaze.expressgateway.controlinterface.node.NodeService;
import com.shieldblaze.expressgateway.controlinterface.node.NodeServiceGrpc;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Server;
import io.grpc.StatusException;
import io.grpc.netty.shaded.io.grpc.netty.NettyServerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.UnknownHostException;
import java.util.Arrays;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class TCPLoadBalancerServiceTest {

    static Server server;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = NettyServerBuilder.forAddress(new InetSocketAddress("127.0.0.1", 9110))
                .addService(new TCPLoadBalancerService())
                .addService(new NodeService())
                .build()
                .start();
    }

    @AfterAll
    static void shutdown() throws InterruptedException {
        server.shutdownNow().awaitTermination();
    }

    @Test
    void simpleServerLBClientTest() throws IOException, InterruptedException {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();

        new TCPServer().start();

        TCPLoadBalancerServiceGrpc.TCPLoadBalancerServiceBlockingStub tcpService = TCPLoadBalancerServiceGrpc.newBlockingStub(channel);
        NodeServiceGrpc.NodeServiceBlockingStub nodeService = NodeServiceGrpc.newBlockingStub(channel);

        Layer4LoadBalancer.TCPLoadBalancer tcpLoadBalancer = Layer4LoadBalancer.TCPLoadBalancer.newBuilder()
                .setBindAddress("127.0.0.1")
                .setBindPort(5000)
                .setName("Meow")
                .build();

        LoadBalancerResponse loadBalancerResponse = tcpService.start(tcpLoadBalancer);
        assertFalse(loadBalancerResponse.getResponseText().isEmpty()); // Load Balancer ID

        NodeOuterClass.addRequest addRequest = NodeOuterClass.addRequest.newBuilder()
                .setAddress("127.0.0.1")
                .setPort(5555)
                .setLoadBalancerID(loadBalancerResponse.getResponseText())
                .setMaxConnections(-1)
                .build();

        NodeOuterClass.addResponse addResponse = nodeService.add(addRequest);
        assertTrue(addResponse.getSuccess());
        assertFalse(addResponse.getNodeId().isEmpty()); // Load Balancer ID

        try (Socket socket = new Socket("127.0.0.1", 5000)) {
            socket.getOutputStream().write("Meow".getBytes());
            assertArrayEquals("Cat".getBytes(), socket.getInputStream().readNBytes(3));
        } catch (IOException e) {
            throw e;
        }

        Thread.sleep(2500L); // Wait for everything to settle down
        channel.shutdownNow();
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
