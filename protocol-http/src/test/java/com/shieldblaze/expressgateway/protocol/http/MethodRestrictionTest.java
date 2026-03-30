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
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * V-TEST-034/V-TEST-035: Method restriction tests.
 *
 * <p>Validates that the load balancer correctly rejects HTTP methods that are
 * not supported or represent security risks when proxied:
 * <ul>
 *   <li><b>H1-05 / V-TEST-034</b>: CONNECT method returns 405 Method Not Allowed.
 *       RFC 9110 Section 9.3.6 defines CONNECT for establishing TCP tunnels. This
 *       L7 proxy does not implement tunneling and must reject it.</li>
 *   <li><b>SPEC-2 / V-TEST-035</b>: TRACE method returns 405 Method Not Allowed.
 *       RFC 9110 Section 9.3.8 defines TRACE for diagnostic loop-back. Proxying
 *       TRACE exposes hop-by-hop headers (cookies, auth tokens) and enables
 *       Cross-Site Tracing (XST) attacks. Nginx and HAProxy reject TRACE by default.</li>
 *   <li><b>SPEC-3</b>: OPTIONS with Max-Forwards: 0 must be handled locally per
 *       RFC 9110 Section 7.6.2 without forwarding to the backend.</li>
 * </ul>
 *
 * <p>Uses raw TCP sockets because Java's HttpClient does not support CONNECT or
 * TRACE in a way that allows us to verify the proxy's response.</p>
 */
@Timeout(value = 30)
class MethodRestrictionTest {

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

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.DEFAULT)
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

        // Brief pause to ensure the server socket is fully ready to accept connections.
        // Without this, the first test connection may arrive before the pipeline is
        // fully initialized, causing an empty response.
        Thread.sleep(500);
    }

    @AfterAll
    static void shutdown() throws Exception {
        httpLoadBalancer.shutdown().future().get();
        httpServer.shutdown();
        httpServer.SHUTDOWN_FUTURE.get();
    }

    // ===================================================================
    // V-TEST-034: CONNECT method rejection
    // ===================================================================

    /**
     * RFC 9110 Section 9.3.6: CONNECT establishes a tunnel to a destination
     * server. This proxy does not support tunneling and MUST reject with 405.
     *
     * <p>The request-target for CONNECT is in authority-form (host:port), not
     * origin-form (path). This is the standard CONNECT request format.</p>
     */
    @Test
    void connectMethod_returns405() throws Exception {
        String rawRequest = "CONNECT www.example.com:443 HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("405"),
                "CONNECT method must return 405 Method Not Allowed but got: " + response);
    }

    /**
     * CONNECT with a path-style URI should also be rejected.
     * Some HTTP smuggling tools use this variant to confuse parsers.
     */
    @Test
    void connectMethodWithPath_returns405() throws Exception {
        String rawRequest = "CONNECT / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        // Either 400 (invalid CONNECT format) or 405 (CONNECT rejected) is acceptable
        assertTrue(response.contains("405") || response.contains("400"),
                "CONNECT with path must return 405 or 400 but got: " + response);
    }

    // ===================================================================
    // V-TEST-035: TRACE method rejection
    // ===================================================================

    /**
     * SPEC-2: TRACE must be rejected with 405 Method Not Allowed.
     *
     * <p>RFC 9110 Section 9.3.8 defines TRACE for diagnostic loop-back. When
     * proxied, it exposes hop-by-hop headers that intermediate proxies may have
     * injected (cookies, authorization tokens), enabling Cross-Site Tracing (XST).
     * Both Nginx and HAProxy reject TRACE by default.</p>
     */
    @Test
    void traceMethod_returns405() throws Exception {
        String rawRequest = "TRACE / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("405"),
                "TRACE method must return 405 Method Not Allowed but got: " + response);
    }

    /**
     * TRACE with request body should also be rejected.
     * Per RFC 9110 Section 9.3.8, TRACE MUST NOT contain a body.
     */
    @Test
    void traceMethodWithBody_returns405() throws Exception {
        String rawRequest = "TRACE / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "Content-Length: 4\r\n" +
                "\r\n" +
                "body";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("405"),
                "TRACE with body must return 405 but got: " + response);
    }

    /**
     * The 405 response for TRACE should include an Allow header listing
     * the methods this proxy does support, per RFC 9110 Section 15.5.6.
     */
    @Test
    void traceMethod_responseContainsAllowHeader() throws Exception {
        String rawRequest = "TRACE / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String fullResponse = readFullResponse(rawRequest);
        assertTrue(fullResponse.contains("405"),
                "TRACE must return 405 but got: " + fullResponse);

        // Check for Allow header (case-insensitive line search)
        boolean hasAllow = fullResponse.lines()
                .anyMatch(line -> line.toLowerCase().startsWith("allow:"));
        assertTrue(hasAllow,
                "405 response to TRACE should include Allow header per RFC 9110 Section 15.5.6");
    }

    // ===================================================================
    // SPEC-3: OPTIONS Max-Forwards handling
    // ===================================================================

    /**
     * RFC 9110 Section 7.6.2: When a proxy receives OPTIONS with Max-Forwards: 0,
     * it MUST NOT forward the request and MUST respond as the final recipient.
     * This test verifies the proxy responds locally with 200 OK.
     */
    @Test
    void optionsWithMaxForwardsZero_handledLocally() throws Exception {
        String rawRequest = "OPTIONS * HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "Max-Forwards: 0\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("200"),
                "OPTIONS with Max-Forwards: 0 must be handled locally with 200 but got: " + response);
    }

    // ===================================================================
    // Positive regression tests: allowed methods still work
    // ===================================================================

    /**
     * GET requests must still be proxied to the backend successfully.
     * This prevents false positives from overly aggressive method filtering.
     */
    @Test
    void getMethod_succeeds() throws Exception {
        String rawRequest = "GET / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("200"),
                "GET must succeed with 200 but got: " + response);
    }

    /**
     * POST requests must still be proxied to the backend successfully.
     */
    @Test
    void postMethod_succeeds() throws Exception {
        String rawRequest = "POST / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "Content-Length: 5\r\n" +
                "\r\n" +
                "hello";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("200"),
                "POST must succeed with 200 but got: " + response);
    }

    // ===================================================================
    // Helpers
    // ===================================================================

    /**
     * Sends a raw HTTP request and returns only the status line.
     */
    private String sendRawRequest(String rawRequest) throws Exception {
        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            socket.setSoTimeout(5000);

            OutputStream out = socket.getOutputStream();
            out.write(rawRequest.getBytes(StandardCharsets.UTF_8));
            out.flush();

            try (BufferedReader reader = new BufferedReader(
                    new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {

                String statusLine = reader.readLine();
                if (statusLine == null) {
                    return "";
                }
                return statusLine;
            }
        }
    }

    /**
     * Sends a raw HTTP request and reads multiple response lines for header inspection.
     */
    private String readFullResponse(String rawRequest) throws Exception {
        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            socket.setSoTimeout(5000);

            OutputStream out = socket.getOutputStream();
            out.write(rawRequest.getBytes(StandardCharsets.UTF_8));
            out.flush();

            try (BufferedReader reader = new BufferedReader(
                    new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {

                List<String> lines = new ArrayList<>();
                String line;
                while ((line = reader.readLine()) != null) {
                    lines.add(line);
                    // Stop after empty line (end of headers)
                    if (line.isEmpty()) {
                        break;
                    }
                }
                return String.join("\n", lines);
            }
        }
    }
}
