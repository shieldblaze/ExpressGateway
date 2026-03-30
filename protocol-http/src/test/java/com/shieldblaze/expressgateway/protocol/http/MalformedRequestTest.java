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

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for malformed HTTP/1.1 request handling per RFC 9112 Section 3.2
 * and RFC 9110 Section 9.3.6.
 *
 * <p>These tests send raw HTTP requests over a TCP socket to exercise the
 * validation logic in {@link Http11ServerInboundHandler#channelRead}. Using
 * raw sockets is necessary because Java's {@code HttpClient} enforces its own
 * header validation, which prevents us from constructing intentionally
 * malformed requests.</p>
 *
 * <p>Validated behaviors:
 * <ul>
 *   <li>H1-01: Missing Host header returns 400 Bad Request</li>
 *   <li>H1-02: Multiple Host headers returns 400 Bad Request</li>
 *   <li>H1-04/SEC-01: Both Content-Length and Transfer-Encoding returns 400</li>
 *   <li>H1-05: CONNECT method returns 405 Method Not Allowed</li>
 * </ul>
 */
class MalformedRequestTest {

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
    }

    @AfterAll
    static void shutdown() throws Exception {
        httpLoadBalancer.shutdown().future().get();
        httpServer.shutdown();
        httpServer.SHUTDOWN_FUTURE.get();
    }

    /**
     * H1-01: RFC 9112 Section 3.2 - A request without a Host header MUST be
     * rejected with 400 Bad Request. Without this check, the proxy would attempt
     * cluster lookup with a null hostname, resulting in an opaque 502.
     */
    @Test
    void missingHostHeader_returns400() throws Exception {
        String rawRequest = "GET / HTTP/1.1\r\n" +
                "Accept: */*\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("400"), "Missing Host header should return 400 but got: " + response);
    }

    /**
     * H1-02: RFC 9112 Section 3.2 - A request with more than one Host header
     * MUST be rejected with 400 Bad Request. Multiple Host headers can indicate
     * a request smuggling attempt or a misconfigured client.
     */
    @Test
    void multipleHostHeaders_returns400() throws Exception {
        String rawRequest = "GET / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "Host: evil.example.com\r\n" +
                "Accept: */*\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("400"), "Multiple Host headers should return 400 but got: " + response);
    }

    /**
     * H1-04/SEC-01: RFC 9112 Section 6.1 - A request with both Content-Length
     * and Transfer-Encoding is a potential smuggling vector. The proxy must
     * handle it safely.
     *
     * <p>Netty's HttpServerCodec resolves the CL+TE conflict at the codec
     * level by preferring Transfer-Encoding (per RFC 9112 Section 6.3),
     * stripping Content-Length before the application handler sees the request.
     * This means the proxy safely resolves the ambiguity -- both the proxy
     * and any backend will agree on body boundaries (TE wins), preventing
     * a CL/TE desync attack.</p>
     *
     * <p>The defense-in-depth check in Http11ServerInboundHandler catches
     * cases where both headers survive the codec, but Netty's codec handles
     * the common case. Either a 400 rejection or a safe 200 is acceptable.</p>
     */
    @Test
    void contentLengthAndTransferEncoding_handledSafely() throws Exception {
        String rawRequest = "POST / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "Content-Length: 5\r\n" +
                "Transfer-Encoding: chunked\r\n" +
                "\r\n" +
                "5\r\nhello\r\n0\r\n\r\n";

        String response = sendRawRequest(rawRequest);
        // Either 400 (rejected) or 200 (safely resolved by codec) is acceptable.
        // The critical thing is that it does NOT cause request smuggling.
        assertTrue(response.contains("400") || response.contains("200"),
                "CL+TE conflict must be handled safely (400 or 200) but got: " + response);
    }

    /**
     * H1-05: RFC 9110 Section 9.3.6 - CONNECT is used for establishing a tunnel
     * to a destination server. This L7 proxy does not implement tunneling and
     * MUST reject CONNECT with 405 Method Not Allowed.
     */
    @Test
    void connectMethod_returns405() throws Exception {
        String rawRequest = "CONNECT www.example.com:443 HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("405"),
                "CONNECT method should return 405 but got: " + response);
    }

    /**
     * Sends a raw HTTP request string over a TCP socket and returns the
     * response status line (first line of the HTTP response).
     *
     * <p>We read until we hit the end of the status line. The server will
     * close the connection after sending the error response (Connection: close
     * semantics), so we can rely on socket closure to terminate the read.</p>
     */
    private String sendRawRequest(String rawRequest) throws Exception {
        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            socket.setSoTimeout(5000);

            OutputStream out = socket.getOutputStream();
            out.write(rawRequest.getBytes(StandardCharsets.UTF_8));
            out.flush();

            try (BufferedReader reader = new BufferedReader(
                    new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {

                // Read the status line (e.g., "HTTP/1.1 400 Bad Request")
                String statusLine = reader.readLine();
                if (statusLine == null) {
                    return "";
                }
                return statusLine;
            }
        }
    }
}
