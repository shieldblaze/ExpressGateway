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
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.backend.Node;
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
import java.nio.charset.StandardCharsets;
import java.util.Random;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

/**
 * P1 integration tests for TCP proxy features.
 *
 * <p>Covers: backpressure under slow-reading backends, TCP half-close (RFC 9293
 * Section 3.6), graceful handling of offline nodes, and round-robin distribution
 * across multiple backend nodes.</p>
 *
 * <p>All backend servers use raw {@link ServerSocket} -- no reactor-netty dependency
 * required in this module.</p>
 */
@Timeout(value = 180, unit = TimeUnit.SECONDS)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
final class TcpProxyTest {

    private static final Random RANDOM = new Random();

    // -- Backend ports (echo servers for round-robin and basic tests) --
    private static final int BACKEND_PORT_1 = AvailablePortUtil.getTcpPort();
    private static final int BACKEND_PORT_2 = AvailablePortUtil.getTcpPort();

    // -- Slow-reading backend for backpressure test --
    private static final int BACKEND_SLOW_PORT = AvailablePortUtil.getTcpPort();

    // -- Half-close backend --
    private static final int BACKEND_HALFCLOSE_PORT = AvailablePortUtil.getTcpPort();

    // -- Load balancer frontend ports --
    private static final int LB_PORT_MAIN = AvailablePortUtil.getTcpPort();
    private static final int LB_PORT_BACKPRESSURE = AvailablePortUtil.getTcpPort();
    private static final int LB_PORT_HALFCLOSE = AvailablePortUtil.getTcpPort();

    // -- Load balancers --
    private static L4LoadBalancer lbMain;
    private static L4LoadBalancer lbBackpressure;
    private static L4LoadBalancer lbHalfclose;

    // -- Backend server threads --
    private static Thread echoServer1;
    private static Thread echoServer2;
    private static Thread slowServer;
    private static Thread halfCloseServer;

    // Counters for connections accepted by each echo backend
    private static final AtomicInteger ECHO1_CONNECTIONS = new AtomicInteger(0);
    private static final AtomicInteger ECHO2_CONNECTIONS = new AtomicInteger(0);

    // Flags to stop backend server accept loops
    private static final AtomicBoolean SERVERS_RUNNING = new AtomicBoolean(true);

    // -----------------------------------------------------------------
    // Backend server implementations
    // -----------------------------------------------------------------

    /**
     * Simple TCP echo server: reads data and echoes it back, one connection at a time
     * in a dedicated thread per accepted socket. Counts accepted connections.
     */
    private static Thread startEchoServer(int port, AtomicInteger connectionCounter) {
        Thread t = new Thread(() -> {
            try (ServerSocket ss = new ServerSocket(port, 50, InetAddress.getByName("127.0.0.1"))) {
                ss.setSoTimeout(1000); // 1s accept timeout for clean shutdown
                while (SERVERS_RUNNING.get()) {
                    Socket client;
                    try {
                        client = ss.accept();
                    } catch (java.net.SocketTimeoutException e) {
                        continue; // loop back and check SERVERS_RUNNING
                    }
                    connectionCounter.incrementAndGet();
                    // Handle each connection in its own thread to allow concurrent connections
                    Thread handler = new Thread(() -> {
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
                            // Connection closed by proxy or client -- expected
                        } finally {
                            try { client.close(); } catch (IOException ignored) { }
                        }
                    }, "echo-handler-" + port + "-" + connectionCounter.get());
                    handler.setDaemon(true);
                    handler.start();
                }
            } catch (IOException e) {
                if (SERVERS_RUNNING.get()) {
                    e.printStackTrace();
                }
            }
        }, "echo-server-" + port);
        t.setDaemon(true);
        t.start();
        return t;
    }

    /**
     * Slow-reading backend: accepts a connection and reads 1 byte at a time with
     * a 5ms delay between reads. This simulates a backend that cannot keep up with
     * the data rate from the proxy, exercising the proxy's backpressure path.
     *
     * <p>The total bytes received are accumulated in {@code totalBytesRead} so the
     * test can verify all data eventually arrives.</p>
     */
    private static final AtomicLong SLOW_BACKEND_BYTES_READ = new AtomicLong(0);

    private static Thread startSlowReadingServer(int port) {
        Thread t = new Thread(() -> {
            try (ServerSocket ss = new ServerSocket(port, 50, InetAddress.getByName("127.0.0.1"))) {
                ss.setSoTimeout(1000);
                while (SERVERS_RUNNING.get()) {
                    Socket client;
                    try {
                        client = ss.accept();
                    } catch (java.net.SocketTimeoutException e) {
                        continue;
                    }
                    // Handle in-thread: we only expect a single connection for this test
                    try {
                        InputStream in = client.getInputStream();
                        while (in.read() != -1) {
                            SLOW_BACKEND_BYTES_READ.incrementAndGet();
                            // Slow down consumption to create backpressure.
                            // 5ms per byte = ~200 bytes/sec, which is *much* slower
                            // than the proxy can write. In practice the proxy's write
                            // buffer will fill quickly and autoRead must be toggled.
                            //
                            // We only do the delay for the first 2048 bytes to avoid
                            // making the test take minutes while still creating enough
                            // pressure to flip the writability flag.
                            if (SLOW_BACKEND_BYTES_READ.get() <= 2048) {
                                Thread.sleep(1);
                            }
                        }
                    } catch (IOException | InterruptedException ignored) {
                    } finally {
                        try { client.close(); } catch (IOException ignored) { }
                    }
                }
            } catch (IOException e) {
                if (SERVERS_RUNNING.get()) {
                    e.printStackTrace();
                }
            }
        }, "slow-server-" + port);
        t.setDaemon(true);
        t.start();
        return t;
    }

    /**
     * Half-close aware backend: reads all data from the client until EOF (the
     * client half-closed), then sends a known response back and closes.
     *
     * <p>This exercises RFC 9293 Section 3.6: after the client sends FIN,
     * the backend should still be able to write data in the opposite direction
     * and the proxy must relay it back to the client.</p>
     */
    private static Thread startHalfCloseServer(int port) {
        Thread t = new Thread(() -> {
            try (ServerSocket ss = new ServerSocket(port, 50, InetAddress.getByName("127.0.0.1"))) {
                ss.setSoTimeout(1000);
                while (SERVERS_RUNNING.get()) {
                    Socket client;
                    try {
                        client = ss.accept();
                    } catch (java.net.SocketTimeoutException e) {
                        continue;
                    }
                    Thread handler = new Thread(() -> {
                        try {
                            InputStream in = client.getInputStream();
                            OutputStream out = client.getOutputStream();

                            // Read all incoming data until EOF (client half-close)
                            byte[] received = in.readAllBytes();

                            // After receiving EOF, send a response that includes the
                            // received byte count so the test can verify end-to-end integrity
                            String response = "HALF_CLOSE_ACK:" + received.length;
                            out.write(response.getBytes(StandardCharsets.UTF_8));
                            out.flush();

                            // Now close our output side (full close from backend)
                            client.close();
                        } catch (IOException ignored) {
                        } finally {
                            try { client.close(); } catch (IOException ignored) { }
                        }
                    }, "halfclose-handler-" + port);
                    handler.setDaemon(true);
                    handler.start();
                }
            } catch (IOException e) {
                if (SERVERS_RUNNING.get()) {
                    e.printStackTrace();
                }
            }
        }, "halfclose-server-" + port);
        t.setDaemon(true);
        t.start();
        return t;
    }

    // -----------------------------------------------------------------
    // Test lifecycle
    // -----------------------------------------------------------------

    @BeforeAll
    static void setup() throws Exception {
        // Start all backend servers
        echoServer1 = startEchoServer(BACKEND_PORT_1, ECHO1_CONNECTIONS);
        echoServer2 = startEchoServer(BACKEND_PORT_2, ECHO2_CONNECTIONS);
        slowServer = startSlowReadingServer(BACKEND_SLOW_PORT);
        halfCloseServer = startHalfCloseServer(BACKEND_HALFCLOSE_PORT);

        // Give servers time to bind
        Thread.sleep(200);

        // ----- Main LB (round-robin over two echo backends) -----
        lbMain = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(new TCPListener())
                .withBindAddress(new InetSocketAddress("127.0.0.1", LB_PORT_MAIN))
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .build();

        Cluster mainCluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        lbMain.defaultCluster(mainCluster);

        NodeBuilder.newBuilder()
                .withCluster(mainCluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", BACKEND_PORT_1))
                .build();

        NodeBuilder.newBuilder()
                .withCluster(mainCluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", BACKEND_PORT_2))
                .build();

        L4FrontListenerStartupTask mainStartup = lbMain.start();
        mainStartup.future().get(10, TimeUnit.SECONDS);
        assertTrue(mainStartup.isSuccess(), "Main LB failed to start");

        // ----- Backpressure LB (single slow-reading backend) -----
        lbBackpressure = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(new TCPListener())
                .withBindAddress(new InetSocketAddress("127.0.0.1", LB_PORT_BACKPRESSURE))
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .build();

        Cluster bpCluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        lbBackpressure.defaultCluster(bpCluster);

        NodeBuilder.newBuilder()
                .withCluster(bpCluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", BACKEND_SLOW_PORT))
                .build();

        L4FrontListenerStartupTask bpStartup = lbBackpressure.start();
        bpStartup.future().get(10, TimeUnit.SECONDS);
        assertTrue(bpStartup.isSuccess(), "Backpressure LB failed to start");

        // ----- Half-close LB (single half-close backend) -----
        lbHalfclose = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(new TCPListener())
                .withBindAddress(new InetSocketAddress("127.0.0.1", LB_PORT_HALFCLOSE))
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .build();

        Cluster hcCluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        lbHalfclose.defaultCluster(hcCluster);

        NodeBuilder.newBuilder()
                .withCluster(hcCluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", BACKEND_HALFCLOSE_PORT))
                .build();

        L4FrontListenerStartupTask hcStartup = lbHalfclose.start();
        hcStartup.future().get(10, TimeUnit.SECONDS);
        assertTrue(hcStartup.isSuccess(), "Half-close LB failed to start");
    }

    @AfterAll
    static void teardown() throws Exception {
        // Signal servers to stop accepting
        SERVERS_RUNNING.set(false);

        // Shut down load balancers (use best-effort shutdown to avoid teardown failures)
        for (var lb : new Object[]{lbMain, lbBackpressure, lbHalfclose}) {
            if (lb != null) {
                try {
                    ((com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer) lb)
                            .shutdown().future().get(10, TimeUnit.SECONDS);
                } catch (java.util.concurrent.TimeoutException e) {
                    // Best-effort: shutdown may take longer with half-close channels
                }
            }
        }

        // Wait for server threads to terminate
        if (echoServer1 != null) echoServer1.join(5000);
        if (echoServer2 != null) echoServer2.join(5000);
        if (slowServer != null) slowServer.join(5000);
        if (halfCloseServer != null) halfCloseServer.join(5000);
    }

    // -----------------------------------------------------------------
    // Test 1: TCP Backpressure
    // -----------------------------------------------------------------

    /**
     * Verifies the proxy handles backpressure without OOM when the backend reads
     * slowly. We send a large chunk of data (10 MB) through the proxy to a backend
     * that reads 1 byte at a time (with a delay for the first 2 KB).
     *
     * <p>The proxy must toggle autoRead on the upstream channel when the downstream
     * write buffer fills (see {@code UpstreamHandler.channelRead} and
     * {@code DownstreamHandler.channelWritabilityChanged}). Without this, Netty's
     * outbound buffer would grow until the JVM runs out of memory.</p>
     *
     * <p>Success criteria: the write completes without error and the backend
     * eventually receives all bytes.</p>
     */
    @Order(1)
    @Test
    void backpressureWithSlowReadingBackend() throws Exception {
        final int dataSize = 10 * 1024 * 1024; // 10 MB

        try (Socket client = new Socket("127.0.0.1", LB_PORT_BACKPRESSURE)) {
            client.setSoTimeout(90_000); // 90 second read timeout
            // Use a large send buffer to push data quickly into the proxy
            client.setSendBufferSize(256 * 1024);

            OutputStream out = client.getOutputStream();

            // Build a 10 MB payload. Use a deterministic pattern so we can verify
            // integrity if needed, but the primary assertion is that the write
            // completes without throwing (no OOM, no broken pipe before all data
            // is accepted by the kernel send buffer + proxy).
            byte[] payload = new byte[dataSize];
            RANDOM.nextBytes(payload);

            // Write in 64 KB chunks to avoid a single massive arraycopy.
            // Socket.getOutputStream().write() will block when the TCP send
            // window fills, which is the client-side manifestation of the
            // backpressure chain: slow backend -> proxy write buffer full ->
            // proxy stops reading from client -> client TCP window closes.
            int offset = 0;
            int chunkSize = 64 * 1024;
            while (offset < dataSize) {
                int len = Math.min(chunkSize, dataSize - offset);
                out.write(payload, offset, len);
                offset += len;
            }
            out.flush();

            // Do NOT shutdownOutput yet -- there may be buffered data in the proxy
            // pipeline that needs to drain to the backend. Just wait for the slow
            // backend to consume all bytes.
            //
            // Wait for the slow backend to drain all bytes. The first 2 KB reads at
            // ~1 byte/ms, the rest runs at full speed. Total time should be well under
            // the 120s test timeout.
            long deadline = System.currentTimeMillis() + 90_000;
            while (SLOW_BACKEND_BYTES_READ.get() < dataSize) {
                if (System.currentTimeMillis() > deadline) {
                    fail("Slow backend did not receive all data within 90s. " +
                            "Received: " + SLOW_BACKEND_BYTES_READ.get() + " / " + dataSize);
                }
                Thread.sleep(500);
            }

            assertTrue(SLOW_BACKEND_BYTES_READ.get() >= dataSize,
                    "Backend should have received at least " + dataSize +
                            " bytes, got " + SLOW_BACKEND_BYTES_READ.get());
        }
    }

    // -----------------------------------------------------------------
    // Test 2: TCP Half-Close (RFC 9293 Section 3.6)
    // -----------------------------------------------------------------

    /**
     * Verifies the proxy correctly handles TCP half-close. Per RFC 9293 Section 3.6,
     * when one side sends FIN, only that direction is closed -- the reverse direction
     * must remain open for data.
     *
     * <p>Sequence:
     * <ol>
     *   <li>Client connects through proxy to the half-close backend</li>
     *   <li>Client sends known data</li>
     *   <li>Client calls {@code shutdownOutput()} -- sends FIN</li>
     *   <li>Backend receives EOF, then sends a response including the byte count</li>
     *   <li>Client reads the response and verifies it</li>
     * </ol>
     *
     * <p>Without ALLOW_HALF_CLOSURE enabled on both the upstream and downstream
     * channels, Netty would treat the received FIN as a full channel close,
     * preventing the response from reaching the client.</p>
     */
    @Order(2)
    @Test
    void halfCloseRelaysDataInBothDirections() throws Exception {
        final String clientData = "Hello from half-close test client!";
        final byte[] clientBytes = clientData.getBytes(StandardCharsets.UTF_8);

        try (Socket client = new Socket("127.0.0.1", LB_PORT_HALFCLOSE)) {
            client.setSoTimeout(30_000);

            OutputStream out = client.getOutputStream();
            InputStream in = client.getInputStream();

            // Step 1: send data to the backend
            out.write(clientBytes);
            out.flush();

            // Give the proxy time to establish the backend connection and forward
            // the data. Without this, the FIN may arrive at the proxy before the
            // backend connection is fully established, and the pending data in the
            // proxy pipeline may be lost when shutdownOutput is relayed.
            Thread.sleep(2000);

            // Step 2: half-close -- sends FIN to the proxy, which should relay
            // it to the backend (shutdownOutput on the downstream channel)
            client.shutdownOutput();

            // Step 3: read the response from the backend (which arrives after
            // the backend sees EOF on its input stream)
            byte[] responseBuf = new byte[4096];
            int totalRead = 0;
            int read;
            while ((read = in.read(responseBuf, totalRead, responseBuf.length - totalRead)) != -1) {
                totalRead += read;
            }

            assertTrue(totalRead > 0, "Expected to receive response from backend after half-close");

            String response = new String(responseBuf, 0, totalRead, StandardCharsets.UTF_8);
            assertEquals("HALF_CLOSE_ACK:" + clientBytes.length, response,
                    "Backend should acknowledge receiving all bytes before half-close");
        }
    }

    // -----------------------------------------------------------------
    // Test 3: Node marked offline during traffic
    // -----------------------------------------------------------------

    /**
     * Verifies that marking a backend node offline is handled gracefully by the proxy.
     *
     * <p>When all nodes are offline, the proxy's {@code UpstreamHandler} receives
     * {@code L4Response.NO_NODE} from the cluster's load balancer and closes the
     * client connection. The client should see EOF (read returns -1) rather than
     * a hang or exception.</p>
     */
    @Order(3)
    @Test
    void nodeOfflineClosesNewConnections() throws Exception {
        // Mark both backend nodes offline
        for (Node node : lbMain.defaultCluster().onlineNodes()) {
            node.markOffline();
        }

        // Small delay for the event to propagate through the EventStream
        Thread.sleep(500);

        try (Socket client = new Socket()) {
            client.connect(new InetSocketAddress("127.0.0.1", LB_PORT_MAIN), 5000);
            client.setSoTimeout(5000);
            assertTrue(client.isConnected(), "Client should connect to the proxy");

            // The proxy should close the connection because there are no online nodes.
            // Allow a short window for the proxy to process the connection and close it.
            // We expect read() to return -1 (EOF) or throw SocketException.
            int result = -1;
            try {
                result = client.getInputStream().read();
            } catch (IOException e) {
                // Connection reset is also acceptable -- the proxy may RST the connection
            }

            assertEquals(-1, result,
                    "Proxy should close the connection when no backend nodes are online");
        } finally {
            // Restore both nodes to online
            for (Node node : lbMain.defaultCluster().allNodes()) {
                node.markOnline();
            }
            // Wait for re-online event propagation
            Thread.sleep(500);
        }
    }

    /**
     * Verifies that existing connections survive when a node is marked offline
     * (connections are not drained unless explicitly requested).
     *
     * <p>This matches the behavior tested in {@code BasicTcpUdpServerTest#sendTcpTrafficInMultiplexingWayAndMarkBackendOfflineWithoutDrainingConnection}:
     * an already-established connection remains functional because the TCP channel
     * to the backend was already opened before the node state changed.</p>
     */
    @Order(4)
    @Test
    void existingConnectionSurvivesNodeOffline() throws Exception {
        final int dataSize = 128;
        final int frames = 100;

        try (Socket client = new Socket("127.0.0.1", LB_PORT_MAIN)) {
            client.setSoTimeout(10_000);
            InputStream in = client.getInputStream();
            OutputStream out = client.getOutputStream();

            // Send a few frames to establish the connection through the proxy
            for (int i = 0; i < 10; i++) {
                byte[] data = new byte[dataSize];
                RANDOM.nextBytes(data);
                out.write(data);
                out.flush();
                byte[] echo = in.readNBytes(dataSize);
                assertArrayEquals(data, echo, "Echo mismatch on warm-up frame " + i);
            }

            // Now mark the first online node offline -- the existing connection
            // should keep working because the downstream channel is already established
            Node firstOnline = lbMain.defaultCluster().onlineNodes().get(0);
            firstOnline.markOffline();

            try {
                // Continue sending on the existing connection
                for (int i = 0; i < frames; i++) {
                    byte[] data = new byte[dataSize];
                    RANDOM.nextBytes(data);
                    out.write(data);
                    out.flush();
                    byte[] echo = in.readNBytes(dataSize);
                    assertArrayEquals(data, echo, "Echo mismatch on frame " + i + " after node marked offline");
                }
            } finally {
                firstOnline.markOnline();
                Thread.sleep(300);
            }
        }
    }

    // -----------------------------------------------------------------
    // Test 4: Multiple backend nodes with RoundRobin
    // -----------------------------------------------------------------

    /**
     * Verifies that traffic is distributed across multiple backend nodes using
     * RoundRobin load balancing. We open multiple independent connections through
     * the proxy and verify that both echo backends receive connections.
     *
     * <p>Note: RoundRobin with NOOPSessionPersistence means each new connection
     * from a unique source address/port gets the next node in rotation. Since we
     * open fresh sockets from ephemeral ports, each connection should alternate.</p>
     */
    @Order(5)
    @Test
    void roundRobinDistributesAcrossBackends() throws Exception {
        // Reset connection counters
        ECHO1_CONNECTIONS.set(0);
        ECHO2_CONNECTIONS.set(0);

        final int totalConnections = 20;
        final int dataSize = 64;
        final CountDownLatch latch = new CountDownLatch(totalConnections);
        final AtomicReference<Throwable> failure = new AtomicReference<>();

        ExecutorService pool = Executors.newFixedThreadPool(totalConnections);

        for (int c = 0; c < totalConnections; c++) {
            pool.submit(() -> {
                try (Socket client = new Socket("127.0.0.1", LB_PORT_MAIN)) {
                    client.setSoTimeout(10_000);
                    InputStream in = client.getInputStream();
                    OutputStream out = client.getOutputStream();

                    // Send one frame and verify echo
                    byte[] data = new byte[dataSize];
                    RANDOM.nextBytes(data);
                    out.write(data);
                    out.flush();

                    byte[] echo = in.readNBytes(dataSize);
                    assertArrayEquals(data, echo, "Echo data mismatch in round-robin test");
                } catch (Throwable t) {
                    failure.compareAndSet(null, t);
                } finally {
                    latch.countDown();
                }
            });
        }

        assertTrue(latch.await(30, TimeUnit.SECONDS), "All connections should complete within 30s");
        pool.shutdown();

        if (failure.get() != null) {
            fail("A connection failed during round-robin test", failure.get());
        }

        // Both backends must have received at least one connection.
        // With 20 connections and RoundRobin, each should get ~10.
        int echo1 = ECHO1_CONNECTIONS.get();
        int echo2 = ECHO2_CONNECTIONS.get();

        assertTrue(echo1 > 0,
                "Echo server 1 should have received at least 1 connection, got " + echo1);
        assertTrue(echo2 > 0,
                "Echo server 2 should have received at least 1 connection, got " + echo2);
        assertEquals(totalConnections, echo1 + echo2,
                "Total connections (" + (echo1 + echo2) + ") should equal " + totalConnections);
    }

    /**
     * Verifies that after restoring all nodes online, traffic still flows correctly
     * end-to-end (sanity check after the offline/online transitions in earlier tests).
     */
    @Order(6)
    @Test
    void trafficFlowsAfterNodeRecovery() throws Exception {
        final int dataSize = 256;
        final int frames = 50;

        try (Socket client = new Socket("127.0.0.1", LB_PORT_MAIN)) {
            client.setSoTimeout(10_000);
            InputStream in = client.getInputStream();
            OutputStream out = client.getOutputStream();

            for (int i = 0; i < frames; i++) {
                byte[] data = new byte[dataSize];
                RANDOM.nextBytes(data);
                out.write(data);
                out.flush();

                byte[] echo = in.readNBytes(dataSize);
                assertArrayEquals(data, echo, "Echo mismatch on recovery frame " + i);
            }
        }
    }
}
