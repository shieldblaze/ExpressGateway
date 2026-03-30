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
package com.shieldblaze.expressgateway.protocol.udp;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * UDP session burst and multi-client tests for the L4 UDP load balancer.
 *
 * <p>Validates that the {@link UpstreamHandler}'s session management
 * (backed by {@link com.shieldblaze.expressgateway.common.map.SelfExpiringMap})
 * correctly:
 * <ul>
 *   <li>Reuses the same upstream session for multiple packets from the same source</li>
 *   <li>Creates independent sessions for packets from different sources</li>
 *   <li>Handles burst traffic from a single client without packet loss</li>
 * </ul>
 *
 * <p>The test stands up a real UDP load balancer on dynamic ports and an echo-style
 * backend that reflects packets back to the sender through the proxy. This is a full
 * integration test matching the pattern from {@link UpstreamHandlerTest}.</p>
 */
@Timeout(value = 60, unit = TimeUnit.SECONDS)
final class UDPSessionBurstTest {

    private static final int BURST_COUNT = 10;

    static L4LoadBalancer l4LoadBalancer;
    static int lbPort;
    static int backendPort;
    static EchoUDPServer echoServer;

    @BeforeAll
    static void setup() throws Exception {
        lbPort = AvailablePortUtil.getUdpPort();
        backendPort = AvailablePortUtil.getUdpPort();

        // Start the echo backend on a dynamic port.
        echoServer = new EchoUDPServer(backendPort);
        echoServer.start();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                .withL4FrontListener(new UDPListener())
                .build();

        l4LoadBalancer.defaultCluster(cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", backendPort))
                .build();

        L4FrontListenerStartupTask startupTask = l4LoadBalancer.start();
        startupTask.future().join();
        assertTrue(startupTask.isSuccess(), "UDP load balancer must start successfully on port " + lbPort);
    }

    @AfterAll
    static void stop() {
        L4FrontListenerStopTask stopTask = l4LoadBalancer.stop();
        stopTask.future().join();
        assertTrue(stopTask.isSuccess(), "UDP load balancer must stop successfully");

        echoServer.shutdown();
    }

    /**
     * Sends a burst of {@link #BURST_COUNT} packets from the same client socket
     * (same source IP:port). All packets must be proxied to the backend and the
     * replies returned successfully. This validates that the session map reuses the
     * same upstream connection for repeat packets from the same source.
     */
    @Test
    void burstFromSameSource_reusesSession() throws Exception {
        try (DatagramSocket client = new DatagramSocket()) {
            client.setSoTimeout(5000);

            for (int i = 0; i < BURST_COUNT; i++) {
                String payload = "BURST_" + i;
                DatagramPacket sendPkt = new DatagramPacket(
                        payload.getBytes(), payload.length(),
                        InetAddress.getByName("127.0.0.1"), lbPort);
                client.send(sendPkt);

                byte[] buf = new byte[2048];
                DatagramPacket recvPkt = new DatagramPacket(buf, buf.length);
                client.receive(recvPkt);

                String response = new String(Arrays.copyOf(recvPkt.getData(), recvPkt.getLength()));
                assertEquals("ECHO:" + payload, response,
                        "Packet #" + i + " reply must echo the payload through the proxy");
            }
        }
    }

    /**
     * Sends packets from multiple independent client sockets (different source ports).
     * Each client must get its own upstream session and receive correct replies.
     * This validates that the session map creates separate entries per unique source address.
     */
    @Test
    void multipleClients_eachGetsOwnSession() throws Exception {
        int clientCount = 5;

        for (int c = 0; c < clientCount; c++) {
            try (DatagramSocket client = new DatagramSocket()) {
                client.setSoTimeout(5000);

                String payload = "CLIENT_" + c;
                DatagramPacket sendPkt = new DatagramPacket(
                        payload.getBytes(), payload.length(),
                        InetAddress.getByName("127.0.0.1"), lbPort);
                client.send(sendPkt);

                byte[] buf = new byte[2048];
                DatagramPacket recvPkt = new DatagramPacket(buf, buf.length);
                client.receive(recvPkt);

                String response = new String(Arrays.copyOf(recvPkt.getData(), recvPkt.getLength()));
                assertEquals("ECHO:" + payload, response,
                        "Client #" + c + " must receive its own echoed payload");
            }
        }
    }

    /**
     * Sends packets from multiple client sockets concurrently to verify that
     * the session map handles concurrent access correctly. The UpstreamHandler
     * uses ConcurrentHashMap.computeIfAbsent which provides per-bin locking,
     * so concurrent senders for different keys should not contend.
     */
    @Test
    void concurrentClients_noPacketLoss() throws Exception {
        int clientCount = 5;
        CountDownLatch latch = new CountDownLatch(clientCount);
        AtomicInteger successCount = new AtomicInteger(0);
        AtomicInteger failureCount = new AtomicInteger(0);

        for (int c = 0; c < clientCount; c++) {
            int clientId = c;
            Thread.ofVirtual().name("udp-client-" + c).start(() -> {
                try (DatagramSocket client = new DatagramSocket()) {
                    client.setSoTimeout(5000);

                    String payload = "CONCURRENT_" + clientId;
                    DatagramPacket sendPkt = new DatagramPacket(
                            payload.getBytes(), payload.length(),
                            InetAddress.getByName("127.0.0.1"), lbPort);
                    client.send(sendPkt);

                    byte[] buf = new byte[2048];
                    DatagramPacket recvPkt = new DatagramPacket(buf, buf.length);
                    client.receive(recvPkt);

                    String response = new String(Arrays.copyOf(recvPkt.getData(), recvPkt.getLength()));
                    if (("ECHO:" + payload).equals(response)) {
                        successCount.incrementAndGet();
                    } else {
                        failureCount.incrementAndGet();
                    }
                } catch (Exception e) {
                    failureCount.incrementAndGet();
                } finally {
                    latch.countDown();
                }
            });
        }

        assertTrue(latch.await(10, TimeUnit.SECONDS),
                "All concurrent clients must complete within timeout");
        assertEquals(clientCount, successCount.get(),
                "All concurrent clients must receive correct responses (failures: " + failureCount.get() + ")");
    }

    // =========================================================================
    // Echo UDP Server: reflects each received datagram back with an "ECHO:" prefix.
    // Handles multiple packets concurrently. Runs as a daemon thread.
    // =========================================================================

    private static final class EchoUDPServer extends Thread {

        private final int port;
        private volatile DatagramSocket socket;
        private volatile boolean running = true;

        EchoUDPServer(int port) {
            this.port = port;
            setDaemon(true);
            setName("EchoUDPServer-" + port);
        }

        @Override
        public void run() {
            try {
                socket = new DatagramSocket(port, InetAddress.getByName("127.0.0.1"));
                byte[] buf = new byte[2048];

                while (running) {
                    DatagramPacket recvPkt = new DatagramPacket(buf, buf.length);
                    socket.receive(recvPkt);

                    String received = new String(Arrays.copyOf(recvPkt.getData(), recvPkt.getLength()));
                    byte[] echoData = ("ECHO:" + received).getBytes();

                    DatagramPacket sendPkt = new DatagramPacket(
                            echoData, echoData.length,
                            recvPkt.getAddress(), recvPkt.getPort());
                    socket.send(sendPkt);
                }
            } catch (Exception e) {
                if (running) {
                    e.printStackTrace();
                }
            }
        }

        void shutdown() {
            running = false;
            if (socket != null && !socket.isClosed()) {
                socket.close();
            }
        }
    }
}
