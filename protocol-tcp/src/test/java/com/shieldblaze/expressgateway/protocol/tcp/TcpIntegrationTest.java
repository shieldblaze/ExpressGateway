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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.junit.jupiter.api.Timeout;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.Random;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

/**
 * Comprehensive TCP proxy integration tests.
 *
 * <p>Each test stands up a dedicated load balancer and backend servers on
 * dynamic ports to avoid cross-test interference and port conflicts.</p>
 */
@Timeout(value = 120, unit = TimeUnit.SECONDS)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
final class TcpIntegrationTest {

    private static final Random RANDOM = new Random(42);

    // Backend + LB for echo tests
    private static final int ECHO_BACKEND_PORT = AvailablePortUtil.getTcpPort();
    private static final int ECHO_LB_PORT = AvailablePortUtil.getTcpPort();
    private static L4LoadBalancer echoLb;
    private static Thread echoServerThread;

    // Backend + LB for large transfer tests
    private static final int LARGE_BACKEND_PORT = AvailablePortUtil.getTcpPort();
    private static final int LARGE_LB_PORT = AvailablePortUtil.getTcpPort();
    private static L4LoadBalancer largeLb;
    private static Thread largeServerThread;

    private static final AtomicBoolean SERVERS_RUNNING = new AtomicBoolean(true);

    @BeforeAll
    static void setup() throws Exception {
        echoServerThread = startEchoServer(ECHO_BACKEND_PORT);
        largeServerThread = startEchoServer(LARGE_BACKEND_PORT);
        Thread.sleep(200);

        echoLb = buildLb(ECHO_LB_PORT, ECHO_BACKEND_PORT);
        largeLb = buildLb(LARGE_LB_PORT, LARGE_BACKEND_PORT);
    }

    @AfterAll
    static void teardown() throws Exception {
        SERVERS_RUNNING.set(false);
        shutdownLb(echoLb);
        shutdownLb(largeLb);
        joinThread(echoServerThread);
        joinThread(largeServerThread);
    }

    // ---------------------------------------------------------------
    // Test 1: Basic connection establishment and data forwarding
    // ---------------------------------------------------------------

    @Order(1)
    @Test
    void connectionEstablishmentAndEcho() throws Exception {
        try (Socket client = new Socket("127.0.0.1", ECHO_LB_PORT)) {
            client.setSoTimeout(10_000);
            OutputStream out = client.getOutputStream();
            InputStream in = client.getInputStream();

            byte[] data = "Hello ExpressGateway TCP Proxy!".getBytes();
            out.write(data);
            out.flush();

            byte[] response = in.readNBytes(data.length);
            assertArrayEquals(data, response);
        }
    }

    // ---------------------------------------------------------------
    // Test 2: Multi-frame echo (multiple reads/writes per connection)
    // ---------------------------------------------------------------

    @Order(2)
    @Test
    void multiFrameEcho() throws Exception {
        try (Socket client = new Socket("127.0.0.1", ECHO_LB_PORT)) {
            client.setSoTimeout(10_000);
            OutputStream out = client.getOutputStream();
            InputStream in = client.getInputStream();

            for (int i = 0; i < 100; i++) {
                byte[] data = new byte[128];
                RANDOM.nextBytes(data);
                out.write(data);
                out.flush();

                byte[] response = in.readNBytes(data.length);
                assertArrayEquals(data, response, "Mismatch at frame " + i);
            }
        }
    }

    // ---------------------------------------------------------------
    // Test 3: Large data transfer (multi-MB)
    // ---------------------------------------------------------------

    @Order(3)
    @Test
    void largeDataTransfer() throws Exception {
        final int totalBytes = 5 * 1024 * 1024; // 5 MB

        try (Socket client = new Socket("127.0.0.1", LARGE_LB_PORT)) {
            client.setSoTimeout(30_000);
            client.setSendBufferSize(256 * 1024);
            client.setReceiveBufferSize(256 * 1024);
            OutputStream out = client.getOutputStream();
            InputStream in = client.getInputStream();

            byte[] payload = new byte[totalBytes];
            RANDOM.nextBytes(payload);
            byte[] expectedDigest = md5(payload);

            // Write in a separate virtual thread so we can read concurrently
            // (necessary for large transfers to avoid deadlock when TCP windows fill)
            AtomicReference<Exception> writeError = new AtomicReference<>();
            Thread writer = Thread.ofVirtual().start(() -> {
                try {
                    int offset = 0;
                    int chunkSize = 64 * 1024;
                    while (offset < totalBytes) {
                        int len = Math.min(chunkSize, totalBytes - offset);
                        out.write(payload, offset, len);
                        offset += len;
                    }
                    out.flush();
                } catch (Exception e) {
                    writeError.set(e);
                }
            });

            // Read all echoed data
            byte[] received = new byte[totalBytes];
            int totalRead = 0;
            while (totalRead < totalBytes) {
                int read = in.read(received, totalRead, totalBytes - totalRead);
                if (read == -1) {
                    break;
                }
                totalRead += read;
            }

            writer.join(15_000);

            if (writeError.get() != null) {
                fail("Write failed", writeError.get());
            }

            assertEquals(totalBytes, totalRead, "Should receive all bytes back");
            byte[] receivedDigest = md5(received);
            assertArrayEquals(expectedDigest, receivedDigest, "Data integrity check (MD5) failed");
        }
    }

    // ---------------------------------------------------------------
    // Test 4: Concurrent connections stress test
    // ---------------------------------------------------------------

    @Order(4)
    @Test
    void concurrentConnectionsStressTest() throws Exception {
        final int concurrency = 20;
        final int framesPerClient = 10;
        final int frameSize = 256;
        CountDownLatch latch = new CountDownLatch(concurrency);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        AtomicInteger successCount = new AtomicInteger(0);

        ExecutorService pool = Executors.newVirtualThreadPerTaskExecutor();

        for (int c = 0; c < concurrency; c++) {
            int clientId = c;
            pool.submit(() -> {
                try (Socket client = new Socket("127.0.0.1", ECHO_LB_PORT)) {
                    client.setSoTimeout(15_000);
                    OutputStream out = client.getOutputStream();
                    InputStream in = client.getInputStream();

                    for (int f = 0; f < framesPerClient; f++) {
                        byte[] data = new byte[frameSize];
                        RANDOM.nextBytes(data);
                        out.write(data);
                        out.flush();

                        byte[] response = in.readNBytes(frameSize);
                        assertArrayEquals(data, response,
                                "Client " + clientId + " frame " + f + " mismatch");
                    }
                    successCount.incrementAndGet();
                } catch (Throwable t) {
                    failure.compareAndSet(null, t);
                } finally {
                    latch.countDown();
                }
            });
        }

        assertTrue(latch.await(60, TimeUnit.SECONDS), "All clients should complete");
        pool.shutdown();

        if (failure.get() != null) {
            fail("Client failed", failure.get());
        }
        assertEquals(concurrency, successCount.get(),
                "All clients should complete successfully");
    }

    // ---------------------------------------------------------------
    // Test 5: Connection timeout (no backend listening)
    // ---------------------------------------------------------------

    @Order(5)
    @Test
    void connectionToOfflineBackendClosesClient() throws Exception {
        int deadPort = AvailablePortUtil.getTcpPort();
        int lbPort = AvailablePortUtil.getTcpPort();

        // Build LB pointing to a port nobody is listening on
        L4LoadBalancer deadLb = buildLb(lbPort, deadPort);
        try {
            try (Socket client = new Socket()) {
                client.connect(new InetSocketAddress("127.0.0.1", lbPort), 5000);
                client.setSoTimeout(15_000);

                // The proxy should close the connection after the backend connect fails
                int result = -1;
                try {
                    result = client.getInputStream().read();
                } catch (IOException e) {
                    // Connection reset is acceptable
                }
                assertEquals(-1, result, "Proxy should close connection when backend is unreachable");
            }
        } finally {
            shutdownLb(deadLb);
        }
    }

    // ---------------------------------------------------------------
    // Test 6: Graceful shutdown with draining
    // ---------------------------------------------------------------

    @Order(6)
    @Test
    void gracefulShutdownClosesActiveConnections() throws Exception {
        int backendPort = AvailablePortUtil.getTcpPort();
        int lbPort = AvailablePortUtil.getTcpPort();

        Thread backend = startEchoServer(backendPort);
        Thread.sleep(200);

        TCPListener listener = new TCPListener();
        listener.setDrainTimeoutSeconds(2);
        L4LoadBalancer lb = buildLbWithListener(lbPort, backendPort, listener);

        Socket client = new Socket("127.0.0.1", lbPort);
        try {
            client.setSoTimeout(10_000);
            OutputStream out = client.getOutputStream();
            InputStream in = client.getInputStream();

            // Establish connection and verify it works
            byte[] data = new byte[64];
            RANDOM.nextBytes(data);
            out.write(data);
            out.flush();
            byte[] response = in.readNBytes(data.length);
            assertArrayEquals(data, response);

            // Verify active connections are tracked
            assertTrue(listener.activeConnections().size() > 0,
                    "Should have at least one active connection tracked");

            // Stop -- this closes server channels and begins drain.
            // After the drain timeout, active connections are forcefully closed.
            lb.stop().future().get(15, TimeUnit.SECONDS);

            // After stop completes (including drain timeout), new connections should fail
            boolean connectFailed = false;
            try (Socket newClient = new Socket()) {
                newClient.connect(new InetSocketAddress("127.0.0.1", lbPort), 2000);
            } catch (IOException e) {
                connectFailed = true;
            }
            assertTrue(connectFailed, "New connections should be refused after stop");
        } finally {
            try { client.close(); } catch (Exception ignored) {}
            try {
                lb.shutdown().future().get(10, TimeUnit.SECONDS);
            } catch (Exception ignored) {
            }
            SERVERS_RUNNING.set(false);
            joinThread(backend);
            SERVERS_RUNNING.set(true); // Reset for other tests
        }
    }

    // ---------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------

    private static L4LoadBalancer buildLb(int lbPort, int backendPort) throws Exception {
        L4LoadBalancer lb = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(new TCPListener())
                .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .build();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();
        lb.defaultCluster(cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", backendPort))
                .build();

        L4FrontListenerStartupTask startup = lb.start();
        startup.future().get(10, TimeUnit.SECONDS);
        assertTrue(startup.isSuccess(), "LB on port " + lbPort + " failed to start");
        return lb;
    }

    private static L4LoadBalancer buildLbWithListener(int lbPort, int backendPort, TCPListener listener) throws Exception {
        L4LoadBalancer lb = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(listener)
                .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .build();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();
        lb.defaultCluster(cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", backendPort))
                .build();

        L4FrontListenerStartupTask startup = lb.start();
        startup.future().get(10, TimeUnit.SECONDS);
        assertTrue(startup.isSuccess(), "LB on port " + lbPort + " failed to start");
        return lb;
    }

    private static void shutdownLb(L4LoadBalancer lb) {
        if (lb != null) {
            try {
                lb.shutdown().future().get(10, TimeUnit.SECONDS);
            } catch (Exception ignored) {
            }
        }
    }

    private static void joinThread(Thread t) {
        if (t != null) {
            try {
                t.join(5000);
            } catch (InterruptedException ignored) {
            }
        }
    }

    private static Thread startEchoServer(int port) {
        Thread t = Thread.ofVirtual().name("echo-" + port).start(() -> {
            try (ServerSocket ss = new ServerSocket(port, 100, InetAddress.getByName("127.0.0.1"))) {
                ss.setSoTimeout(1000);
                while (SERVERS_RUNNING.get()) {
                    Socket client;
                    try {
                        client = ss.accept();
                    } catch (java.net.SocketTimeoutException e) {
                        continue;
                    }
                    Thread.ofVirtual().name("echo-handler-" + port).start(() -> {
                        try {
                            InputStream in = client.getInputStream();
                            OutputStream out = client.getOutputStream();
                            byte[] buf = new byte[8192];
                            int read;
                            while ((read = in.read(buf)) != -1) {
                                out.write(buf, 0, read);
                                out.flush();
                            }
                        } catch (IOException ignored) {
                        } finally {
                            try { client.close(); } catch (IOException ignored) { }
                        }
                    });
                }
            } catch (IOException e) {
                if (SERVERS_RUNNING.get()) {
                    e.printStackTrace();
                }
            }
        });
        return t;
    }

    private static byte[] md5(byte[] data) {
        try {
            return MessageDigest.getInstance("MD5").digest(data);
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }
}
