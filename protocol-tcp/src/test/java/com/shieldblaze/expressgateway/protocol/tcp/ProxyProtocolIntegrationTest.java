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
import com.shieldblaze.expressgateway.configuration.transport.BackendProxyProtocolMode;
import com.shieldblaze.expressgateway.configuration.transport.ProxyProtocolMode;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
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
import java.util.Arrays;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * End-to-end integration tests for outbound PROXY protocol encoding.
 *
 * <p>Each test connects through a shared load balancer (one per mode: V1, V2, OFF)
 * to a plain Java {@link ServerSocket}-based backend that manually parses the PROXY
 * protocol header (v1 or v2) sent by the proxy, captures the decoded source address,
 * and echoes subsequent application bytes back.</p>
 *
 * <p>The test then asserts:</p>
 * <ol>
 *   <li>The backend received a correctly formatted PROXY protocol header.</li>
 *   <li>The client IP encoded in the header matches the actual connecting client
 *       (127.0.0.1 for these loopback tests).</li>
 *   <li>Echo data flows correctly after the header is consumed.</li>
 * </ol>
 *
 * <p>Using a plain {@code ServerSocket} backend avoids any Netty pipeline
 * complexity (e.g., {@code ByteToMessageDecoder} replay-after-remove semantics)
 * and gives us direct byte-level control to assert the exact wire format.</p>
 *
 * <p>All LBs and backends are created once in {@code @BeforeAll} and torn down in
 * {@code @AfterAll} to avoid per-test NIO EventLoopGroup creation overhead, which
 * causes test isolation failures when many groups are created and destroyed in
 * rapid succession within a single JVM.</p>
 */
@Timeout(value = 60, unit = TimeUnit.SECONDS)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
final class ProxyProtocolIntegrationTest {

    /** Binary signature for PROXY protocol v2 (12 bytes) */
    private static final byte[] V2_SIG = {
            0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A
    };

    // -- Shared infrastructure created once for the entire test class --
    private static ProxyProtocolBackend backendV1;
    private static ProxyProtocolBackend backendV2;
    private static PlainEchoBackend     backendOff;

    private static L4LoadBalancer lbV1;
    private static L4LoadBalancer lbV2;
    private static L4LoadBalancer lbOff;

    // -- Frontend ports (allocated at class-load time, before any LB starts) --
    private static final int LB_PORT_V1  = AvailablePortUtil.getTcpPort();
    private static final int LB_PORT_V2  = AvailablePortUtil.getTcpPort();
    private static final int LB_PORT_OFF = AvailablePortUtil.getTcpPort();

    // -------------------------------------------------------------------------
    // Class lifecycle
    // -------------------------------------------------------------------------

    @BeforeAll
    static void setup() throws Exception {
        // Start all three backends first so ports are bound before LBs connect
        backendV1  = new ProxyProtocolBackend(BackendProxyProtocolMode.V1);
        backendV2  = new ProxyProtocolBackend(BackendProxyProtocolMode.V2);
        backendOff = new PlainEchoBackend();

        backendV1.start();
        backendV2.start();
        backendOff.start();

        // Build V1 load balancer
        lbV1 = buildLb(backendV1.port(), BackendProxyProtocolMode.V1, LB_PORT_V1);

        // Build V2 load balancer
        lbV2 = buildLb(backendV2.port(), BackendProxyProtocolMode.V2, LB_PORT_V2);

        // Build OFF load balancer (uses DEFAULT config; no PP header injected)
        TCPListener offListener = new TCPListener();
        offListener.setDrainTimeoutSeconds(0);
        lbOff = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(offListener)
                .withBindAddress(new InetSocketAddress("127.0.0.1", LB_PORT_OFF))
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .build();

        Cluster offCluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();
        lbOff.defaultCluster(offCluster);
        NodeBuilder.newBuilder()
                .withCluster(offCluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", backendOff.port()))
                .build();

        L4FrontListenerStartupTask offStartup = lbOff.start();
        offStartup.future().get(10, TimeUnit.SECONDS);
        assertTrue(offStartup.isSuccess(), "OFF-mode LB must start");
    }

    @AfterAll
    static void teardown() throws Exception {
        for (L4LoadBalancer lb : new L4LoadBalancer[]{ lbV1, lbV2, lbOff }) {
            if (lb != null) {
                try { lb.shutdown().future().get(10, TimeUnit.SECONDS); }
                catch (Exception ignored) { }
            }
        }

        if (backendV1  != null) backendV1.stop();
        if (backendV2  != null) backendV2.stop();
        if (backendOff != null) backendOff.stop();
    }

    // -------------------------------------------------------------------------
    // Test 1: PROXY protocol v1 header is sent and decoded correctly
    // -------------------------------------------------------------------------

    /**
     * Verifies that when {@code backendProxyProtocolMode=V1}, the proxy sends a
     * well-formed PROXY protocol v1 text header ({@code PROXY TCP4 ...}) as the
     * first bytes on the backend connection, and the backend can parse the
     * source address from it.
     *
     * <p>The encoded source IP must be {@code 127.0.0.1} (the test client's
     * loopback address) and the echo must return correctly.</p>
     */
    @Order(1)
    @Test
    void v1HeaderEncodedAndDecodedByBackend() throws Exception {
        CompletableFuture<InetSocketAddress> addrFuture = backendV1.nextAddressFuture();

        try (Socket client = new Socket("127.0.0.1", LB_PORT_V1)) {
            client.setSoTimeout(10_000);
            OutputStream out = client.getOutputStream();
            InputStream  in  = client.getInputStream();

            byte[] payload = "HelloProxyProtocolV1".getBytes(StandardCharsets.UTF_8);
            out.write(payload);
            out.flush();

            byte[] echo = in.readNBytes(payload.length);
            assertEquals("HelloProxyProtocolV1", new String(echo, StandardCharsets.UTF_8),
                    "Echo data must pass through the proxy unchanged after v1 PP header");

            InetSocketAddress realAddress = addrFuture.get(5, TimeUnit.SECONDS);
            assertNotNull(realAddress,
                    "Backend must have decoded a real client address from the PROXY v1 header");
            assertEquals("127.0.0.1", realAddress.getAddress().getHostAddress(),
                    "Encoded source IP must be the client's loopback address");
            assertTrue(realAddress.getPort() > 0,
                    "Encoded source port must be positive");
        }
    }

    // -------------------------------------------------------------------------
    // Test 2: PROXY protocol v2 header is sent and decoded correctly
    // -------------------------------------------------------------------------

    /**
     * Verifies that when {@code backendProxyProtocolMode=V2}, the proxy sends a
     * well-formed PROXY protocol v2 binary header and the backend can parse the
     * source address from the 12-byte signature + fixed header + IPv4 address block.
     */
    @Order(2)
    @Test
    void v2HeaderEncodedAndDecodedByBackend() throws Exception {
        CompletableFuture<InetSocketAddress> addrFuture = backendV2.nextAddressFuture();

        try (Socket client = new Socket("127.0.0.1", LB_PORT_V2)) {
            client.setSoTimeout(10_000);
            OutputStream out = client.getOutputStream();
            InputStream  in  = client.getInputStream();

            byte[] payload = "HelloProxyProtocolV2".getBytes(StandardCharsets.UTF_8);
            out.write(payload);
            out.flush();

            byte[] echo = in.readNBytes(payload.length);
            assertEquals("HelloProxyProtocolV2", new String(echo, StandardCharsets.UTF_8),
                    "Echo data must pass through the proxy unchanged after v2 PP header");

            InetSocketAddress realAddress = addrFuture.get(5, TimeUnit.SECONDS);
            assertNotNull(realAddress,
                    "Backend must have decoded a real client address from the PROXY v2 header");
            assertEquals("127.0.0.1", realAddress.getAddress().getHostAddress(),
                    "Encoded source IP must be the client's loopback address");
            assertTrue(realAddress.getPort() > 0,
                    "Encoded source port must be positive");
        }
    }

    // -------------------------------------------------------------------------
    // Test 3: OFF mode — no header is sent, plain echo works
    // -------------------------------------------------------------------------

    /**
     * Baseline: with {@code backendProxyProtocolMode=OFF} (the default), no PROXY
     * protocol header is injected. The backend is a plain TCP echo server.
     * Data must still flow correctly.
     */
    @Order(3)
    @Test
    void offModeNoHeaderSentPlainEchoWorks() throws Exception {
        try (Socket client = new Socket("127.0.0.1", LB_PORT_OFF)) {
            client.setSoTimeout(10_000);
            OutputStream out = client.getOutputStream();
            InputStream  in  = client.getInputStream();

            byte[] payload = "NoPPHeader".getBytes(StandardCharsets.UTF_8);
            out.write(payload);
            out.flush();

            byte[] echo = in.readNBytes(payload.length);
            assertEquals("NoPPHeader", new String(echo, StandardCharsets.UTF_8),
                    "Echo must work with no PROXY protocol header (OFF mode)");
        }
    }

    // -------------------------------------------------------------------------
    // Test 4: Multiple frames after PP header — encoder removes itself cleanly
    // -------------------------------------------------------------------------

    /**
     * Verifies that after the one-shot encoder fires on connect and removes
     * itself from the pipeline, subsequent writes from the client flow as plain
     * application data. The echo loop must work across multiple frames.
     *
     * <p>This guards against the encoder accidentally remaining in the pipeline
     * or interfering with data after connection establishment.</p>
     */
    @Order(4)
    @Test
    void multipleFramesAfterV1Header() throws Exception {
        CompletableFuture<InetSocketAddress> addrFuture = backendV1.nextAddressFuture();

        try (Socket client = new Socket("127.0.0.1", LB_PORT_V1)) {
            client.setSoTimeout(15_000);
            OutputStream out = client.getOutputStream();
            InputStream  in  = client.getInputStream();

            for (int i = 1; i <= 5; i++) {
                byte[] frame = ("Frame" + i).getBytes(StandardCharsets.UTF_8);
                out.write(frame);
                out.flush();
                byte[] echo = in.readNBytes(frame.length);
                assertEquals("Frame" + i, new String(echo, StandardCharsets.UTF_8),
                        "Frame " + i + " must echo correctly");
            }

            // Address must still have been captured from the PP header on connect
            InetSocketAddress realAddress = addrFuture.get(5, TimeUnit.SECONDS);
            assertNotNull(realAddress, "Real client address must have been decoded from v1 header");
        }
    }

    // -------------------------------------------------------------------------
    // Load balancer factory
    // -------------------------------------------------------------------------

    private static L4LoadBalancer buildLb(int backendPort, BackendProxyProtocolMode ppMode,
                                          int lbPort) throws Exception {
        TransportConfiguration transportConfig = new TransportConfiguration()
                .transportType(TransportType.NIO)
                .receiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .receiveBufferSizes(new int[]{512, 9001, 65535})
                .tcpConnectionBacklog(1024)
                .socketReceiveBufferSize(65536)
                .socketSendBufferSize(65536)
                .tcpFastOpenMaximumPendingRequests(100)
                .backendConnectTimeout(5_000)
                .connectionIdleTimeout(60_000)
                .proxyProtocolMode(ProxyProtocolMode.OFF)
                .backendProxyProtocolMode(ppMode)
                .validate();

        ConfigurationContext configCtx = ConfigurationContext.create(transportConfig);

        // Use drain timeout of 0 seconds so @AfterAll teardown completes promptly.
        TCPListener listener = new TCPListener();
        listener.setDrainTimeoutSeconds(0);

        L4LoadBalancer lb = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(listener)
                .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                .withCoreConfiguration(configCtx)
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
        assertTrue(startup.isSuccess(), "LB must start for PP mode " + ppMode);
        return lb;
    }

    // -------------------------------------------------------------------------
    // Backend: plain ServerSocket that manually parses PP headers
    // -------------------------------------------------------------------------

    /**
     * Plain Java {@link ServerSocket} backend that accepts multiple connections,
     * manually parses the PROXY protocol header (v1 text or v2 binary) on each
     * connection, exposes the decoded source address via a per-connection
     * {@link CompletableFuture} (fetched before connecting), and echoes all
     * remaining bytes back to the sender.
     *
     * <p>The {@link #nextAddressFuture()} method must be called <em>before</em>
     * opening the client connection to ensure the future is registered before the
     * backend accepts it. The backend uses a {@link BlockingQueue} of pre-allocated
     * futures; each accepted connection polls one future to complete.</p>
     *
     * <p>Using raw sockets avoids any Netty pipeline complexity. The header parsing
     * gives us byte-level control to assert the exact wire format produced by the
     * encoder.</p>
     */
    private static final class ProxyProtocolBackend {

        private final BackendProxyProtocolMode expectedMode;
        private final AtomicBoolean running = new AtomicBoolean(true);

        /**
         * Queue of futures pre-registered by test methods via {@link #nextAddressFuture()}.
         * The accept loop polls one future per accepted connection and completes it
         * with the decoded address (or exceptionally on parse error).
         */
        private final BlockingQueue<CompletableFuture<InetSocketAddress>> pendingFutures =
                new LinkedBlockingQueue<>();

        private ServerSocket serverSocket;
        private Thread acceptThread;
        private int port;

        ProxyProtocolBackend(BackendProxyProtocolMode expectedMode) {
            this.expectedMode = expectedMode;
        }

        /**
         * Returns a fresh {@link CompletableFuture} that will be completed with the
         * source address decoded from the next accepted connection's PP header.
         *
         * <p>Must be called <em>before</em> the client connects to ensure the future
         * is in the queue when the backend's accept loop picks up the connection.</p>
         */
        CompletableFuture<InetSocketAddress> nextAddressFuture() {
            CompletableFuture<InetSocketAddress> future = new CompletableFuture<>();
            pendingFutures.add(future);
            return future;
        }

        int port() {
            return port;
        }

        void start() throws IOException {
            serverSocket = new ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"));
            serverSocket.setSoTimeout(2_000);
            port = serverSocket.getLocalPort();

            acceptThread = Thread.ofVirtual().name("pp-backend-" + port).start(() -> {
                while (running.get()) {
                    Socket conn;
                    try {
                        conn = serverSocket.accept();
                    } catch (java.net.SocketTimeoutException e) {
                        continue;
                    } catch (IOException e) {
                        if (running.get()) e.printStackTrace();
                        return;
                    }
                    // Poll the next registered future (non-blocking — test must register
                    // it before connecting; if none is registered use a dummy future so
                    // the connection is still handled without blocking the accept loop).
                    CompletableFuture<InetSocketAddress> future = pendingFutures.poll();
                    if (future == null) {
                        future = new CompletableFuture<>(); // discard — no test registered
                    }
                    final CompletableFuture<InetSocketAddress> f = future;
                    Thread.ofVirtual().name("pp-handler-" + port).start(() -> handle(conn, f));
                }
            });
        }

        private void handle(Socket conn, CompletableFuture<InetSocketAddress> addrFuture) {
            try {
                conn.setSoTimeout(10_000);
                InputStream  in  = conn.getInputStream();
                OutputStream out = conn.getOutputStream();

                // Read first 12 bytes to distinguish v1 from v2
                byte[] peek = readExactly(in, V2_SIG.length);
                InetSocketAddress addr;

                if (Arrays.equals(peek, V2_SIG)) {
                    addr = parseV2Header(in, peek);
                } else {
                    // V1: peek is the first 12 bytes of the "PROXY ..." line
                    addr = parseV1Header(in, peek);
                }
                addrFuture.complete(addr);

                // Echo all remaining bytes
                byte[] buf = new byte[8192];
                int n;
                while ((n = in.read(buf)) != -1) {
                    out.write(buf, 0, n);
                    out.flush();
                }
            } catch (Exception e) {
                addrFuture.completeExceptionally(e);
            } finally {
                try { conn.close(); } catch (IOException ignored) { }
            }
        }

        /**
         * Parse PROXY protocol v1. We already read the first 12 bytes (stored in
         * {@code initialBytes}); read until {@code \r\n} and then parse.
         */
        private InetSocketAddress parseV1Header(InputStream in, byte[] initialBytes) throws IOException {
            // Buffer the initial 12 bytes plus more until we find \r\n
            byte[] lineBuf = new byte[108];
            System.arraycopy(initialBytes, 0, lineBuf, 0, initialBytes.length);
            int pos = initialBytes.length;

            // Read one byte at a time until \r\n is found
            while (pos < lineBuf.length - 1) {
                int b = in.read();
                if (b == -1) break;
                lineBuf[pos++] = (byte) b;
                if (pos >= 2 && lineBuf[pos - 2] == '\r' && lineBuf[pos - 1] == '\n') {
                    break;
                }
            }

            // Parse: "PROXY TCP4 srcIP dstIP srcPort dstPort\r\n"
            String line = new String(lineBuf, 0, pos, StandardCharsets.US_ASCII).trim();
            String[] parts = line.split(" ");
            // parts[0]=PROXY, parts[1]=TCP4/TCP6/UNKNOWN, parts[2]=srcIP, parts[3]=dstIP,
            // parts[4]=srcPort, parts[5]=dstPort
            if (parts.length >= 6 && !parts[1].equals("UNKNOWN")) {
                try {
                    String srcIp = parts[2];
                    int srcPort = Integer.parseInt(parts[4]);
                    return new InetSocketAddress(srcIp, srcPort);
                } catch (NumberFormatException e) {
                    return null; // malformed port
                }
            }
            return null; // UNKNOWN or malformed
        }

        /**
         * Parse PROXY protocol v2. We already read the 12-byte signature (stored
         * in {@code sig}); read the remaining 4 fixed bytes then the address block.
         *
         * <pre>
         * Byte 12:    verCmd (0x21 = version2/PROXY, 0x20 = version2/LOCAL)
         * Byte 13:    family (0x11 = AF_INET/TCP, 0x21 = AF_INET6/TCP, 0x00 = UNSPEC)
         * Byte 14-15: addrLen (big-endian)
         * Byte 16+:   address data
         * </pre>
         */
        private InetSocketAddress parseV2Header(InputStream in, byte[] sig) throws IOException {
            // Read 4 more bytes: verCmd, family, addrLen (2 bytes)
            byte[] fixed = readExactly(in, 4);
            int verCmd  = fixed[0] & 0xFF;
            int family  = fixed[1] & 0xFF;
            int addrLen = ((fixed[2] & 0xFF) << 8) | (fixed[3] & 0xFF);

            byte[] addrData = readExactly(in, addrLen);

            int command = verCmd & 0x0F;
            int af = (family >> 4) & 0x0F;

            if (command == 0x01 /* PROXY */ && af == 0x01 /* AF_INET */ && addrLen >= 12) {
                // IPv4: 4 bytes src, 4 bytes dst, 2 bytes srcPort, 2 bytes dstPort
                String srcIp = (addrData[0] & 0xFF) + "." + (addrData[1] & 0xFF) + "." +
                        (addrData[2] & 0xFF) + "." + (addrData[3] & 0xFF);
                int srcPort = ((addrData[8] & 0xFF) << 8) | (addrData[9] & 0xFF);
                return new InetSocketAddress(srcIp, srcPort);
            } else if (command == 0x01 && af == 0x02 && addrLen >= 36) {
                // IPv6: 16 bytes src, 16 bytes dst, 2 bytes srcPort, 2 bytes dstPort
                StringBuilder sb = new StringBuilder();
                for (int i = 0; i < 16; i += 2) {
                    if (i > 0) sb.append(':');
                    sb.append(String.format("%02x%02x", addrData[i] & 0xFF, addrData[i + 1] & 0xFF));
                }
                int srcPort = ((addrData[32] & 0xFF) << 8) | (addrData[33] & 0xFF);
                return new InetSocketAddress(sb.toString(), srcPort);
            }
            // LOCAL command or UNSPEC — no address
            return null;
        }

        private static byte[] readExactly(InputStream in, int count) throws IOException {
            byte[] buf = new byte[count];
            int read = 0;
            while (read < count) {
                int n = in.read(buf, read, count - read);
                if (n == -1) throw new IOException("Stream ended before reading " + count + " bytes (read " + read + ")");
                read += n;
            }
            return buf;
        }

        void stop() {
            running.set(false);
            try { serverSocket.close(); } catch (IOException ignored) { }
            if (acceptThread != null) {
                try { acceptThread.join(3_000); } catch (InterruptedException ignored) { }
            }
        }
    }

    // -------------------------------------------------------------------------
    // Plain echo backend (no PP header parsing)
    // -------------------------------------------------------------------------

    private static final class PlainEchoBackend {

        private final AtomicBoolean running = new AtomicBoolean(true);
        private ServerSocket serverSocket;
        private Thread acceptThread;
        private int port;

        int port() {
            return port;
        }

        void start() throws IOException {
            serverSocket = new ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"));
            serverSocket.setSoTimeout(1_000);
            port = serverSocket.getLocalPort();

            acceptThread = Thread.ofVirtual().name("echo-backend-" + port).start(() -> {
                while (running.get()) {
                    Socket conn;
                    try {
                        conn = serverSocket.accept();
                    } catch (java.net.SocketTimeoutException e) {
                        continue;
                    } catch (IOException e) {
                        if (running.get()) e.printStackTrace();
                        return;
                    }
                    Thread.ofVirtual().start(() -> {
                        try {
                            InputStream  in  = conn.getInputStream();
                            OutputStream out = conn.getOutputStream();
                            byte[] buf = new byte[8192];
                            int n;
                            while ((n = in.read(buf)) != -1) {
                                out.write(buf, 0, n);
                                out.flush();
                            }
                        } catch (IOException ignored) {
                        } finally {
                            try { conn.close(); } catch (IOException ignored) { }
                        }
                    });
                }
            });
        }

        void stop() {
            running.set(false);
            try { serverSocket.close(); } catch (IOException ignored) { }
            if (acceptThread != null) {
                try { acceptThread.join(3_000); } catch (InterruptedException ignored) { }
            }
        }
    }
}
