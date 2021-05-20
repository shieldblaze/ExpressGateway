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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.UnknownHostException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class UpstreamHandlerTest {

    static L4LoadBalancer l4LoadBalancer;

    @BeforeAll
    static void setup() throws UnknownHostException {
        new TCPServer().start();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress("127.0.0.1", 9110))
                .withL4FrontListener(new TCPListener())
                .build();

        l4LoadBalancer.defaultCluster(cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", 9111))
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = l4LoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccess());
    }

    @AfterAll
    static void stop() {
        L4FrontListenerStopEvent l4FrontListenerStopEvent = l4LoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccess());
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
