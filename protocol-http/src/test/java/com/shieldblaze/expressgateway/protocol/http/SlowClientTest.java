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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.lang.reflect.Constructor;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.SocketException;
import java.net.SocketTimeoutException;
import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * V-TEST-030/V-TEST-031: Slow client attack defense tests.
 *
 * <p>Validates two distinct slow-client DoS attack vectors:
 * <ul>
 *   <li><b>V-TEST-030 / SEC-02: Slowloris (slow headers)</b> -- Attacker opens a TCP
 *       connection and sends HTTP headers very slowly, one byte at a time. The existing
 *       idle timeout never fires because data IS being sent. The
 *       {@link RequestHeaderTimeoutHandler} enforces an absolute deadline for the first
 *       complete set of HTTP request headers, analogous to Nginx's
 *       {@code client_header_timeout}.</li>
 *   <li><b>V-TEST-031 / ME-04: Slow POST (slow body)</b> -- Attacker sends valid headers
 *       normally, then trickles the request body at 1 byte/second. The idle timeout
 *       resets on each channelRead. The {@link RequestBodyTimeoutHandler} enforces an
 *       absolute deadline for receiving the complete body after headers arrive, analogous
 *       to Apache's {@code RequestReadTimeout body=N}.</li>
 * </ul>
 *
 * <p>The load balancer is configured with deliberately short timeouts (2 seconds for
 * headers, 3 seconds for body) to make the tests fast and deterministic. The
 * {@link HttpConfiguration} constructor is package-private, so this test uses
 * reflection to create a custom configuration -- standard practice for testing
 * package-private constructors from a different package.</p>
 */
@Timeout(value = 30)
class SlowClientTest {

    private static final Logger logger = LogManager.getLogger(SlowClientTest.class);

    /**
     * Short header timeout for testing -- 2 seconds.
     * The real default is 30 seconds.
     */
    private static final long HEADER_TIMEOUT_SECONDS = 2;

    /**
     * Short body timeout for testing -- 3 seconds.
     * The real default is 60 seconds.
     */
    private static final long BODY_TIMEOUT_SECONDS = 3;

    private static int loadBalancerPort;
    private static HTTPLoadBalancer httpLoadBalancer;
    private static HttpServer httpServer;

    @BeforeAll
    static void setup() throws Exception {
        httpServer = new HttpServer(false);
        httpServer.start();
        httpServer.START_FUTURE.get();

        loadBalancerPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        // Create a custom HttpConfiguration with short timeouts via reflection.
        // The constructor is package-private to prevent outside initialization,
        // but tests need to create custom configurations with short timeouts.
        HttpConfiguration shortTimeoutConfig = createShortTimeoutConfig();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(shortTimeoutConfig))
                .withBindAddress(new InetSocketAddress("127.0.0.1", loadBalancerPort))
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mappedCluster("localhost:" + loadBalancerPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                .build();

        L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
        startupTask.future().join();
        assertTrue(startupTask.isSuccess());

        Thread.sleep(200);
    }

    @AfterAll
    static void shutdown() throws Exception {
        httpLoadBalancer.shutdown().future().get();
        httpServer.shutdown();
        httpServer.SHUTDOWN_FUTURE.get();
    }

    /**
     * Creates an HttpConfiguration with short timeouts via reflection.
     * Mirrors the field values from HttpConfiguration.DEFAULT except for
     * requestHeaderTimeoutSeconds and requestBodyTimeoutSeconds.
     */
    private static HttpConfiguration createShortTimeoutConfig() throws Exception {
        Constructor<HttpConfiguration> ctor = HttpConfiguration.class.getDeclaredConstructor();
        ctor.setAccessible(true);
        HttpConfiguration config = ctor.newInstance();

        config.maxInitialLineLength(4096)
                .maxHeaderSize(8192)
                .maxChunkSize(8192)
                .compressionThreshold(1024)
                .deflateCompressionLevel(6)
                .brotliCompressionLevel(4)
                .maxConcurrentStreams(100)
                .backendResponseTimeoutSeconds(60)
                .maxRequestBodySize(10L * 1024 * 1024)
                .initialWindowSize(1048576)
                .h2ConnectionWindowSize(1048576)
                .maxConnectionBodySize(256L * 1024 * 1024)
                .maxHeaderListSize(8192)
                .requestHeaderTimeoutSeconds(HEADER_TIMEOUT_SECONDS)
                .requestBodyTimeoutSeconds(BODY_TIMEOUT_SECONDS)
                .maxH1ConnectionsPerNode(32)
                .maxH2ConnectionsPerNode(4)
                .poolIdleTimeoutSeconds(60)
                .gracefulShutdownDrainMs(5000)
                .validate();

        return config;
    }

    // ===================================================================
    // V-TEST-030: Slowloris (slow headers)
    // ===================================================================

    /**
     * SEC-02: Simulates a Slowloris attack by sending HTTP headers one byte
     * at a time with long pauses. The proxy's RequestHeaderTimeoutHandler should
     * close the connection after HEADER_TIMEOUT_SECONDS.
     *
     * <p>Attack pattern: open connection, send partial headers slowly, never
     * complete the header block. The connection should be killed by the absolute
     * deadline timer before the attacker can tie it up indefinitely.</p>
     */
    @Test
    void slowHeaders_connectionClosedByTimeout() throws Exception {
        long startTime = System.currentTimeMillis();
        boolean connectionClosed = false;

        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            // Set a generous read timeout so we can detect the server-side close
            socket.setSoTimeout((int) (HEADER_TIMEOUT_SECONDS * 1000 + 5000));

            OutputStream out = socket.getOutputStream();

            // Send request line and partial headers, one byte at a time
            String partialHeaders = "GET / HTTP/1.1\r\nHost: localhost:" + loadBalancerPort + "\r\n";
            // Do NOT send the final \r\n that would complete the headers

            for (byte b : partialHeaders.getBytes(StandardCharsets.UTF_8)) {
                try {
                    out.write(b);
                    out.flush();
                    // 200ms between bytes -- slow enough to demonstrate the attack
                    // but fast enough that the test completes quickly
                    Thread.sleep(200);
                } catch (SocketException e) {
                    // Connection closed by server -- this is the expected behavior
                    connectionClosed = true;
                    break;
                }
            }

            if (!connectionClosed) {
                // Headers partially sent. Now just wait -- the server should close
                // the connection after HEADER_TIMEOUT_SECONDS.
                try (BufferedReader reader = new BufferedReader(
                        new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {
                    String line = reader.readLine();
                    // If we get null, the server closed the connection
                    if (line == null) {
                        connectionClosed = true;
                    }
                    // If we get a response, that is also acceptable (server may have
                    // sent a response before closing)
                } catch (SocketException | SocketTimeoutException e) {
                    connectionClosed = true;
                }
            }
        }

        long elapsed = System.currentTimeMillis() - startTime;

        assertTrue(connectionClosed,
                "Slowloris connection must be closed by the server after header timeout");

        // The connection should be closed within a reasonable margin of the timeout.
        // Allow up to 2x the timeout + margin for scheduling jitter.
        long maxExpectedMs = HEADER_TIMEOUT_SECONDS * 1000 * 2 + 3000;
        assertTrue(elapsed < maxExpectedMs,
                "Slowloris timeout should fire within " + maxExpectedMs + "ms but took " + elapsed + "ms");

        logger.info("Slowloris defense: connection closed after {}ms (timeout: {}s)",
                elapsed, HEADER_TIMEOUT_SECONDS);
    }

    /**
     * Regression test: A client that sends complete headers quickly should
     * NOT be killed by the header timeout.
     */
    @Test
    void fastHeaders_connectionNotClosed() throws Exception {
        String rawRequest = "GET / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            socket.setSoTimeout(5000);

            OutputStream out = socket.getOutputStream();
            out.write(rawRequest.getBytes(StandardCharsets.UTF_8));
            out.flush();

            try (BufferedReader reader = new BufferedReader(
                    new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {

                String statusLine = reader.readLine();
                assertTrue(statusLine != null && statusLine.contains("200"),
                        "Fast headers must get a 200 response but got: " + statusLine);
            }
        }
    }

    // ===================================================================
    // V-TEST-031: Slow POST (slow body)
    // ===================================================================

    /**
     * ME-04: Simulates a slow-POST attack by sending valid headers with a
     * Content-Length, then trickling the body at 1 byte per second.
     * The proxy's RequestBodyTimeoutHandler should close the connection
     * (with 408 Request Timeout) after BODY_TIMEOUT_SECONDS.
     *
     * <p>Attack pattern: send valid "POST / HTTP/1.1" with "Content-Length: 1000",
     * then send body bytes slowly. The idle timeout does not help because
     * data IS being sent, resetting the idle timer. The absolute body deadline
     * timer handles this.</p>
     */
    @Test
    void slowBody_connectionClosedByTimeout() throws Exception {
        long startTime = System.currentTimeMillis();
        boolean connectionClosedOrTimedOut = false;
        String response = null;

        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            // Generous read timeout -- we expect the server to act first
            socket.setSoTimeout((int) (BODY_TIMEOUT_SECONDS * 1000 + 5000));

            OutputStream out = socket.getOutputStream();

            // Send complete headers with a large Content-Length
            String headers = "POST / HTTP/1.1\r\n" +
                    "Host: localhost:" + loadBalancerPort + "\r\n" +
                    "Content-Length: 1000\r\n" +
                    "\r\n";

            out.write(headers.getBytes(StandardCharsets.UTF_8));
            out.flush();

            // Trickle body bytes -- send only 5 bytes total, one per second
            for (int i = 0; i < 5; i++) {
                try {
                    Thread.sleep(1000);
                    out.write('A');
                    out.flush();
                } catch (SocketException e) {
                    connectionClosedOrTimedOut = true;
                    break;
                }
            }

            if (!connectionClosedOrTimedOut) {
                // Wait for server to close the connection or send 408
                try (BufferedReader reader = new BufferedReader(
                        new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {
                    String line = reader.readLine();
                    if (line == null) {
                        connectionClosedOrTimedOut = true;
                    } else {
                        response = line;
                        // 408 Request Timeout is the expected response
                        connectionClosedOrTimedOut = line.contains("408") || line.contains("400");
                    }
                } catch (SocketException | SocketTimeoutException e) {
                    connectionClosedOrTimedOut = true;
                }
            }
        }

        long elapsed = System.currentTimeMillis() - startTime;

        assertTrue(connectionClosedOrTimedOut,
                "Slow POST connection must be closed or receive 408 after body timeout. " +
                        "Response was: " + response);

        // Connection should close within a reasonable margin of the body timeout
        long maxExpectedMs = BODY_TIMEOUT_SECONDS * 1000 * 2 + 5000;
        assertTrue(elapsed < maxExpectedMs,
                "Slow POST timeout should fire within " + maxExpectedMs + "ms but took " + elapsed + "ms");

        logger.info("Slow POST defense: connection handled after {}ms (timeout: {}s), response: {}",
                elapsed, BODY_TIMEOUT_SECONDS, response);
    }

    /**
     * Regression test: A client that sends the complete body quickly should
     * NOT be affected by the body timeout.
     */
    @Test
    void fastBody_succeeds() throws Exception {
        String body = "hello";
        String rawRequest = "POST / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "Content-Length: " + body.length() + "\r\n" +
                "\r\n" +
                body;

        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            socket.setSoTimeout(5000);

            OutputStream out = socket.getOutputStream();
            out.write(rawRequest.getBytes(StandardCharsets.UTF_8));
            out.flush();

            try (BufferedReader reader = new BufferedReader(
                    new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {

                String statusLine = reader.readLine();
                assertTrue(statusLine != null && statusLine.contains("200"),
                        "Fast body must get a 200 response but got: " + statusLine);
            }
        }
    }
}
